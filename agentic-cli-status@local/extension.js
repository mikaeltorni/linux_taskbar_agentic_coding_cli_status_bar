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
// CLIs (Codex, Claude Code, Grok CLI, OpenCode Go, OpenRouter). The extension renders
// ONLY: all data
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
  ['opencode_go', 'OG'],
  ['openrouter', 'OR'],
];

// Percent-LEFT gradient stops. Every rendered percentage gets its own
// color: 100% left is green, ~50% left is yellow, 1% left is orange.
// Interpolation is linear per RGB channel (full 8-bit precision) within each
// half, routed through the yellow midpoint so mid values read as a warm
// yellow instead of the washed-out near-white a straight green→orange lerp
// produces. 0% left snaps to pure red. Dashes/separators use neutral gray.
const GRADIENT_FULL_RGB = [0x8f, 0xf0, 0xa4];
const GRADIENT_MID_RGB = [0xf8, 0xe4, 0x5c];
const GRADIENT_LOW_RGB = [0xff, 0x7b, 0x63];
const EMPTY_COLOR = '#ff0000';
const NEUTRAL_COLOR = '#9a9996';

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
 * Whether a window's reset moment has already elapsed.
 *
 * The poller only sees usage the CLIs record locally, so after a boot with no
 * fresh activity the newest reading can be hours old and still name a reset
 * that has since passed. Such a window has rolled over.
 *
 * @param {object} window - One window entry ({resets_at, ...}).
 * @param {number} nowEpoch - Current epoch seconds.
 * @returns {boolean} True when resets_at is present and no longer in the future.
 */
function resetPassed(window, nowEpoch) {
  return Boolean(window?.resets_at) && Number(window.resets_at) <= nowEpoch;
}

/**
 * Compute the whole percent LEFT for one rate-limit window, clamped 0-100.
 *
 * A window whose reset has already passed has replenished: the CLI resets it
 * at resets_at, so with no newer usage recorded it is fully available again.
 * Report it as 100% left rather than echoing the stale pre-reset exhaustion.
 *
 * @param {object} window - One window entry ({used_percent, ...}).
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {number} Remaining budget in percent, rounded to an integer.
 */
function percentLeft(window, nowEpoch = GLib.get_real_time() / 1000000) {
  if (resetPassed(window, nowEpoch))
    return 100;
  const used = window?.used_percent ?? 0;
  return Math.round(Math.min(100, Math.max(0, 100 - used)));
}

/**
 * Map a percent-LEFT value to its gradient color.
 *
 * 100% left is green (GRADIENT_FULL_RGB), the midpoint is yellow
 * (GRADIENT_MID_RGB), 1% left is orange (GRADIENT_LOW_RGB). Each half
 * interpolates linearly per RGB channel; routing through yellow keeps mid
 * values vivid instead of collapsing toward white. 0% left is pure red
 * (EMPTY_COLOR).
 *
 * @param {number} left - Percent remaining, 0-100.
 * @returns {string} "#rrggbb" hex color.
 */
function percentColor(left) {
  if (left < 1)
    return EMPTY_COLOR;
  const clamped = Math.min(left, 100);
  const mid = (1 + 100) / 2;
  const [from, to, t] = clamped <= mid
    ? [GRADIENT_LOW_RGB, GRADIENT_MID_RGB, (clamped - 1) / (mid - 1)]
    : [GRADIENT_MID_RGB, GRADIENT_FULL_RGB, (clamped - mid) / (100 - mid)];
  const hex = from.map((start, i) => {
    const channel = Math.round(start + (to[i] - start) * t);
    return channel.toString(16).padStart(2, '0');
  }).join('');
  return `#${hex}`;
}

/**
 * Format an exhausted weekly window's reset as a local weekday and clock.
 *
 * @param {?number} epoch - Reset time in epoch seconds.
 * Endpoint reset timestamps can jitter by a fraction of a minute, so rounding
 * to the nearest minute keeps both panel and menu clocks stable.
 *
 * @returns {string} Localized reset such as "Mon 14:30", or "0" when absent.
 */
function resetClock(epoch) {
  if (!epoch)
    return '0';
  const roundedEpoch = Math.round(Number(epoch) / 60) * 60;
  return GLib.DateTime.new_from_unix_local(roundedEpoch).format('%a %H:%M');
}

/**
 * Format an exhausted monthly window's reset as a local calendar date.
 *
 * @param {?number} epoch - Reset time in epoch seconds.
 * @returns {string} Localized date such as "8 Aug", or "0" when absent.
 */
function resetDate(epoch) {
  if (!epoch)
    return '0';
  const roundedEpoch = Math.round(Number(epoch) / 60) * 60;
  return GLib.DateTime.new_from_unix_local(roundedEpoch).format('%e %b').trim();
}

/**
 * Format the time remaining until a rolling-window reset as hours/minutes.
 *
 * @param {?number} epoch - Reset time in epoch seconds.
 * @param {number} nowEpoch - Current epoch seconds.
 * @returns {string} Compact duration such as "4h59m", clamped at "0h00m".
 */
function resetCountdown(epoch, nowEpoch) {
  if (!epoch)
    return '0';
  const minutes = Math.max(0, Math.ceil((epoch - nowEpoch) / 60));
  const hours = Math.floor(minutes / 60);
  if (hours === 0)
    return `${minutes}m`;
  return `${hours}h${String(minutes % 60).padStart(2, '0')}m`;
}

/**
 * Format the compact taskbar Pango markup for one CLI, e.g. "CX 43·57".
 *
 * Percentages shown are the limits LEFT (remaining), matching how the CLIs
 * themselves phrase it ("Weekly limit left"), not the used share. Each
 * window's number is colored by its own gradient value; the tag follows the
 * first displayed window. An exhausted weekly limit replaces all values with
 * its red local reset weekday/time. When only the 5h limit is exhausted, its
 * red reset countdown replaces zero and the remaining weekly values follow
 * with hyphen separators. Normal separators and the no-data dash are neutral.
 * The "%" sign is omitted to reduce clutter. A window whose reset moment has
 * already passed counts as replenished (see :func:`percentLeft`), so a stale
 * boot reading flips from a dead "0m"/reset clock back to its full value. An
 * errored snapshot is rendered as unknown instead of showing its carried
 * percentages, because a subscription or account change can invalidate them.
 *
 * @param {string} tag - Short CLI tag.
 * @param {object} snapshot - One CLI entry from status.json.
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {string} Segment markup; a trailing "?" marks stale data.
 */
function segmentMarkup(tag, snapshot, nowEpoch = GLib.get_real_time() / 1000000) {
  const windows = snapshot?.windows ?? [];
  if (!windows.length)
    return `<span foreground="${NEUTRAL_COLOR}">${tag} –</span>`;
  if (snapshot.error) {
    const marker = snapshot.stale ? '?' : '!';
    return `<span foreground="${NEUTRAL_COLOR}">${tag} –${marker}</span>`;
  }

  const weekly = windows.find(w => w.label === 'week');
  if (weekly && percentLeft(weekly, nowEpoch) === 0) {
    const staleMark = snapshot.stale ? '?' : '';
    return `<span foreground="${EMPTY_COLOR}">${tag} </span>` +
      `<span foreground="${EMPTY_COLOR}">` +
      `${resetClock(weekly.resets_at)}${staleMark}</span>`;
  }

  const shortTerm = windows.find(w => w.label === '5h' || w.label === 'rolling');
  const fiveHourExhausted = shortTerm && percentLeft(shortTerm, nowEpoch) === 0;
  const tagColor = fiveHourExhausted
    ? EMPTY_COLOR
    : percentColor(percentLeft(windows[0], nowEpoch));
  const separator = fiveHourExhausted ? '-' : '·';
  const parts = windows
    .map(w => {
      const left = percentLeft(w, nowEpoch);
      if (w === shortTerm && fiveHourExhausted) {
        return `<span foreground="${EMPTY_COLOR}">` +
          `${resetCountdown(w.resets_at, nowEpoch)}</span>`;
      }
      if (w !== weekly && left === 0) {
        const reset = w.label === 'month'
          ? resetDate(w.resets_at)
          : resetClock(w.resets_at);
        return `<span foreground="${EMPTY_COLOR}">` +
          `${reset}</span>`;
      }
      return `<span foreground="${percentColor(left)}">${left}</span>`;
    })
    .join(`<span foreground="${NEUTRAL_COLOR}">${separator}</span>`);
  const staleMark = snapshot.stale
    ? `<span foreground="${tagColor}">?</span>`
    : '';
  return `<span foreground="${tagColor}">${tag} </span>${parts}${staleMark}`;
}

/**
 * Join independent Codex account segments with a gray pipe.
 *
 * Agent Command Center registers extra ChatGPT logins as isolated
 * ``CODEX_HOME`` directories. Each account's windows stay independent; the
 * first keeps the CX tag and later ones use their numeric id so the bar
 * scales as instances are added: ``CX 43·57 | 2 12``.
 *
 * @param {string} tag - Short CLI tag for the primary account.
 * @param {object} snapshot - Codex CLI entry from status.json.
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {string} Combined segment markup.
 */
function codexBarMarkup(tag, snapshot, nowEpoch = GLib.get_real_time() / 1000000) {
  const instances = snapshot?.instances;
  if (!Array.isArray(instances) || instances.length < 2)
    return segmentMarkup(tag, snapshot, nowEpoch);
  const sep = `<span foreground="${NEUTRAL_COLOR}"> | </span>`;
  return instances.map((instance, index) => {
    const instanceTag = index === 0 ? tag : String(instance?.id ?? index + 1);
    return segmentMarkup(instanceTag, instance, nowEpoch);
  }).join(sep);
}

/**
 * Details-menu heading for one Codex account instance.
 *
 * @param {object} instance - One entry from snapshot.instances.
 * @returns {string} Heading such as "Codex [2] trial".
 */
function codexInstanceTitle(instance) {
  const id = instance?.id ?? '?';
  const label = String(instance?.label ?? '').trim();
  return label ? `Codex [${id}] ${label}` : `Codex [${id}]`;
}

/**
 * Format an epoch reset time for the details menu in the local timezone.
 *
 * @param {?number} epoch - Reset time in epoch seconds.
 * @param {?string} label - Window label; monthly windows use a calendar date.
 * @returns {string} Localized "resets …" suffix, or an empty string.
 */
function resetsText(epoch, label) {
  if (!epoch)
    return '';
  const reset = label === 'month' ? resetDate(epoch) : resetClock(epoch);
  return ` — resets ${reset}`;
}

/** Format Codex's optional manual-reset allowance for the details heading. */
function resetAvailabilityText(count) {
  return Number.isInteger(count) && count > 0
    ? ` — ${count} reset${count === 1 ? '' : 's'} available`
    : '';
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
    const names = {
      codex: 'Codex',
      claude_code: 'Claude Code',
      grok: 'Grok CLI',
      opencode_go: 'OpenCode Go',
      openrouter: 'OpenRouter',
    };
    for (const [cliId, tag] of CLI_TAGS) {
      const snapshot = status.clis?.[cliId];
      const label = this._segments.get(cliId);
      if (!snapshot || snapshot.enabled === false) {
        label.visible = false;
        continue;
      }
      label.visible = true;
      label.clutter_text.set_markup(
        cliId === 'codex'
          ? codexBarMarkup(tag, snapshot)
          : segmentMarkup(tag, snapshot));
      label.style_class = snapshot.stale
        ? 'acs-segment acs-stale'
        : 'acs-segment';
      if (cliId === 'codex' && Array.isArray(snapshot.instances) &&
          snapshot.instances.length > 1) {
        for (const instance of snapshot.instances) {
          if (!this.menu.isEmpty())
            this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
          this._appendMenuSection(codexInstanceTitle(instance), instance);
        }
        continue;
      }
      if (!this.menu.isEmpty())
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
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
    // A carried plan can belong to the subscription state from before an
    // endpoint error; do not present it as current while the reading is
    // unknown.
    const plan = snapshot.error ? '' : (snapshot.plan ? ` (${snapshot.plan})` : '');
    const resetCount = resetAvailabilityText(snapshot.resets_available);
    this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
      `${name}${plan}${resetCount}`, { reactive: false }));
    if (snapshot.error) {
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        `   ⚠ ${snapshot.error}`, { reactive: false }));
      return;
    }
    const nowEpoch = GLib.get_real_time() / 1000000;
    for (const window of snapshot.windows ?? []) {
      const left = percentLeft(window, nowEpoch);
      // A window that has already rolled over reads as full; its old reset
      // time is history, so don't dangle it as if still upcoming.
      const reset = resetPassed(window, nowEpoch)
        ? '' : resetsText(window.resets_at, window.label);
      const item = new PopupMenu.PopupMenuItem('', { reactive: false });
      item.label.clutter_text.set_markup(
        `   ${GLib.markup_escape_text(String(window.label ?? ''), -1)}: ` +
        `<span foreground="${percentColor(left)}">${left}% left</span>` +
        GLib.markup_escape_text(reset, -1));
      this.menu.addMenuItem(item);
    }
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
