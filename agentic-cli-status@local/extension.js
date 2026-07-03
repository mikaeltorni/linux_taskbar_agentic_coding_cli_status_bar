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
// file with a Gio file monitor (plus a slow fallback timer) and never talks
// to any CLI or network itself, so it can never consume plan limits.
//
// Components:
//   - log()                  Centralized, level-aware journal logging helper.
//   - AgenticCliStatusButton Panel button: per-CLI colored segments + details menu.
//   - AgenticCliStatusExtension  Lifecycle: enable/disable, file monitor, timer.

const FALLBACK_REFRESH_SECONDS = 60;
const LOG_PREFIX = '[agentic-cli-status]';

// Short taskbar tags per CLI id, in display order.
const CLI_TAGS = [
  ['codex', 'CX'],
  ['claude_code', 'CC'],
  ['grok', 'GK'],
];

// Usage bands mapped to stylesheet classes.
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
 * Format the compact taskbar text for one CLI, e.g. "CX 57·43%".
 *
 * @param {string} tag - Short CLI tag.
 * @param {object} snapshot - One CLI entry from status.json.
 * @returns {string} Segment text; a trailing "?" marks stale data.
 */
function segmentText(tag, snapshot) {
  const windows = snapshot?.windows ?? [];
  if (!windows.length)
    return `${tag} –`;
  const parts = windows.map(w => `${Math.round(w.used_percent ?? 0)}`).join('·');
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
        `   ${window.label}: ${Math.round(window.used_percent ?? 0)}% used` +
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
    // Slow fallback timer: catches missed monitor events and recomputes
    // staleness display even when the file does not change.
    this._timerId = GLib.timeout_add_seconds(
      GLib.PRIORITY_DEFAULT, FALLBACK_REFRESH_SECONDS, () => {
        this._refresh();
        return GLib.SOURCE_CONTINUE;
      });
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
   * Read status.json and re-render the indicator (best-effort).
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
  }
}
