import St from 'gi://St';
import Gio from 'gi://Gio';
import GObject from 'gi://GObject';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

// extension.js — agentic-cli-status@local
//
// Top-bar indicator showing the rate-limit usage of the major agentic coding
// CLIs (Codex, Claude Code, Grok CLI). The extension renders ONLY: all data
// collection happens in the agentic-cli-status poller service, which writes
// ~/.local/state/agentic-cli-status/status.json. The extension watches that
// file with a Gio file monitor plus a fallback timer paced at the poller's
// own configured rate (poll_interval_seconds, set by the installer's
// polling_rate component and echoed into status.json), and never talks to
// any CLI or network itself, so it can never consume plan limits.
//
// Components:
//   - log()                  Centralized, level-aware journal logging helper.
//   - AgenticCliStatusButton Panel button: per-CLI colored segments + details menu.
//   - AgenticCliStatusExtension  Lifecycle: enable/disable, file monitor, timer.

// Fallback-timer bounds mirror the poller's poll_interval_seconds validation
// (5-3600 s); the default matches the poller's default rate so the bar keeps
// the configured cadence even when file-monitor events are missed or the
// status file does not exist yet.
const MIN_REFRESH_SECONDS = 5;
const MAX_REFRESH_SECONDS = 3600;
const DEFAULT_REFRESH_SECONDS = 5;
const LOG_PREFIX = '[agentic-cli-status]';

// Short taskbar tags per CLI id, in display order.
const CLI_TAGS = [
  ['codex', 'CX'],
  ['claude_code', 'CC'],
  ['grok', 'GK'],
];

// Usage bands mapped to stylesheet classes (thresholds are in percent USED;
// the rendered numbers are percent LEFT, so crit = 15% or less left).
const WARN_PERCENT = 60;
const CRIT_PERCENT = 85;

// Ordered log levels; everything at or above ACTIVE_LOG_LEVEL is emitted.
const LOG_LEVELS = { verbose: 0, debug: 1, info: 2, warn: 3, error: 4 };
const ACTIVE_LOG_LEVEL = LOG_LEVELS.info;

/**
 * Centralized logging helper for the extension.
 *
 * Emits a single, prefixed, level-tagged line to the GNOME Shell journal
 * (viewable via `journalctl --user -o cat /usr/bin/gnome-shell`).
 *
 * @param {('verbose'|'debug'|'info'|'warn'|'error')} level - Severity.
 * @param {string} message - Human-readable, searchable log message.
 * @returns {void}
 */
function log(level, message) {
  if ((LOG_LEVELS[level] ?? LOG_LEVELS.info) < ACTIVE_LOG_LEVEL)
    return;
  console.log(`${LOG_PREFIX} [${level}] ${message}`);
}

/**
 * Resolve the poller's status file path honoring XDG_STATE_HOME.
 *
 * @returns {string} Absolute path to status.json.
 */
function statusFilePath() {
  const stateHome = GLib.getenv('XDG_STATE_HOME') ||
    GLib.build_filenamev([GLib.get_home_dir(), '.local', 'state']);
  return GLib.build_filenamev([stateHome, 'agentic-cli-status', 'status.json']);
}

/**
 * Pick the stylesheet severity class for a CLI snapshot.
 *
 * @param {object} snapshot - One CLI entry from status.json.
 * @returns {string} One of the acs-* style classes.
 */
function severityClass(snapshot) {
  const windows = snapshot?.windows ?? [];
  if (!windows.length)
    return 'acs-off';
  const worst = Math.max(...windows.map(w => w.used_percent ?? 0));
  if (worst >= CRIT_PERCENT)
    return 'acs-crit';
  if (worst >= WARN_PERCENT)
    return 'acs-warn';
  return 'acs-ok';
}

/**
 * Format the compact taskbar text for one CLI, e.g. "CX 43·57%".
 *
 * Percentages shown are the limits LEFT (remaining), matching how the CLIs
 * themselves phrase it ("Weekly limit left"), not the used share.
 *
 * @param {string} tag - Short CLI tag.
 * @param {object} snapshot - One CLI entry from status.json.
 * @returns {string} Segment text; a trailing "?" marks stale data.
 */
function segmentText(tag, snapshot) {
  const windows = snapshot?.windows ?? [];
  if (!windows.length)
    return `${tag} –`;
  const parts = windows
    .map(w => `${Math.round(100 - (w.used_percent ?? 0))}`)
    .join('·');
  return `${tag} ${parts}%${snapshot.stale ? '?' : ''}`;
}

/**
 * Format an epoch reset time for the details menu in the local timezone.
 *
 * @param {?number} epoch - Reset time in epoch seconds.
 * @returns {string} Localized "resets …" suffix, or an empty string.
 */
function resetsText(epoch) {
  if (!epoch)
    return '';
  const date = GLib.DateTime.new_from_unix_local(epoch);
  return ` — resets ${date.format('%a %H:%M')}`;
}

const AgenticCliStatusButton = GObject.registerClass(
class AgenticCliStatusButton extends PanelMenu.Button {
  _init() {
    super._init(0.5, 'Agentic CLI Status');
    this._box = new St.BoxLayout({ y_align: Clutter.ActorAlign.CENTER });
    this._segments = new Map();
    for (const [cliId, tag] of CLI_TAGS) {
      const label = new St.Label({
        text: `${tag} –`,
        y_align: Clutter.ActorAlign.CENTER,
        style_class: 'acs-segment acs-off',
      });
      this._segments.set(cliId, label);
      this._box.add_child(label);
    }
    this.add_child(this._box);
  }

  /**
   * Re-render the taskbar segments and the popup menu from a status object.
   *
   * @param {?object} status - Parsed status.json content, or null when the
   *   poller has not produced data yet.
   * @returns {void}
   */
  render(status) {
    this.menu.removeAll();
    if (!status) {
      for (const [cliId, tag] of CLI_TAGS) {
        const label = this._segments.get(cliId);
        label.text = `${tag} –`;
        label.style_class = 'acs-segment acs-off';
        label.visible = true;
      }
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        'Waiting for the agentic-cli-status poller…', { reactive: false }));
      return;
    }
    const names = { codex: 'Codex', claude_code: 'Claude Code', grok: 'Grok CLI' };
    for (const [cliId, tag] of CLI_TAGS) {
      const snapshot = status.clis?.[cliId];
      const label = this._segments.get(cliId);
      if (!snapshot || snapshot.enabled === false) {
        label.visible = false;
        continue;
      }
      label.visible = true;
      label.text = segmentText(tag, snapshot);
      const classes = ['acs-segment', severityClass(snapshot)];
      if (snapshot.stale)
        classes.push('acs-stale');
      label.style_class = classes.join(' ');
      this._appendMenuSection(names[cliId], snapshot);
    }
  }

  /**
   * Append one CLI's detail lines to the popup menu.
   *
   * @param {string} name - Human-readable CLI name.
   * @param {object} snapshot - One CLI entry from status.json.
   * @returns {void}
   */
  _appendMenuSection(name, snapshot) {
    const plan = snapshot.plan ? ` (${snapshot.plan})` : '';
    this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
      `${name}${plan}`, { reactive: false }));
    for (const window of snapshot.windows ?? []) {
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        `   ${window.label}: ${Math.round(100 - (window.used_percent ?? 0))}% left` +
        resetsText(window.resets_at),
        { reactive: false }));
    }
    if (snapshot.error) {
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        `   ⚠ ${snapshot.error}`, { reactive: false }));
    }
    this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
  }
});

export default class AgenticCliStatusExtension extends Extension {
  enable() {
    log('info', 'enabling');
    this._button = new AgenticCliStatusButton();
    Main.panel.addToStatusArea('agentic-cli-status', this._button);
    this._statusFile = Gio.File.new_for_path(statusFilePath());
    try {
      this._monitor = this._statusFile.monitor_file(Gio.FileMonitorFlags.NONE, null);
      this._monitorId = this._monitor.connect('changed', () => this._refresh());
    } catch (error) {
      log('warn', `file monitor unavailable, timer only: ${error}`);
      this._monitor = null;
    }
    this._timerId = null;
    this._timerSeconds = 0;
    this._refresh();
  }

  disable() {
    log('info', 'disabling');
    if (this._timerId) {
      GLib.source_remove(this._timerId);
      this._timerId = null;
    }
    if (this._monitor) {
      this._monitor.disconnect(this._monitorId);
      this._monitor.cancel();
      this._monitor = null;
    }
    this._statusFile = null;
    this._button?.destroy();
    this._button = null;
  }

  /**
   * Read status.json, re-render the indicator, and re-arm the fallback
   * timer at the poller's configured rate (best-effort).
   *
   * @returns {void}
   */
  _refresh() {
    let status = null;
    try {
      const [ok, bytes] = this._statusFile.load_contents(null);
      if (ok)
        status = JSON.parse(new TextDecoder().decode(bytes));
    } catch (error) {
      log('debug', `status file unreadable: ${error}`);
    }
    try {
      this._button.render(status);
    } catch (error) {
      log('error', `render failed: ${error}`);
    }
    this._scheduleRefresh(status);
  }

  /**
   * (Re)arm the one-shot fallback refresh timer.
   *
   * The delay follows the poller's configured rate: status.json carries the
   * installer-configurable poll_interval_seconds, clamped here to the same
   * 5-3600 s range the poller validates. Every refresh (file-monitor event
   * or timer firing) pushes the timer back, so it only actually fires when
   * a status write was missed — guaranteeing the bar is never more than one
   * configured interval behind.
   *
   * @param {?object} status - Parsed status.json content, or null.
   * @returns {void}
   */
  _scheduleRefresh(status) {
    const configured = Number(status?.poll_interval_seconds);
    const seconds = Number.isFinite(configured)
      ? Math.min(MAX_REFRESH_SECONDS,
        Math.max(MIN_REFRESH_SECONDS, Math.round(configured)))
      : DEFAULT_REFRESH_SECONDS;
    if (seconds !== this._timerSeconds) {
      log('info', `fallback refresh timer set to ${seconds}s`);
      this._timerSeconds = seconds;
    }
    if (this._timerId)
      GLib.source_remove(this._timerId);
    this._timerId = GLib.timeout_add_seconds(
      GLib.PRIORITY_DEFAULT, seconds, () => {
        this._timerId = null;
        this._refresh();
        return GLib.SOURCE_REMOVE;
      });
  }
}
