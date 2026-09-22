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
// CLIs (Codex, Claude Code, Grok CLI, OpenCode Go, OpenRouter) plus the
// Cursor spending-page scrape (CRS). The extension renders
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
  ['cursor', 'CRS'],
];

// Poller-side default enablement, mirroring `default_config()` in src/config.rs.
// Claude Code, OpenCode Go, OpenRouter, and Cursor are opt-in: their
// segments must stay off the bar until the user enables them, including
// before the first status.json write.
const DEFAULT_CLI_ENABLED = {
  codex: true,
  claude_code: false,
  grok: true,
  opencode_go: false,
  openrouter: false,
  cursor: false,
};

// Percent-LEFT gradient stops. Every rendered percentage gets its own
// color: 100% left is green, ~50% left is yellow, 1% left is orange.
// Interpolation is linear per RGB channel (full 8-bit precision) within each
// half, routed through the yellow midpoint so mid values read as a warm
// yellow instead of the washed-out near-white a straight green→orange lerp
// produces. 0% left snaps to pure red. Dashes/separators use neutral gray.
const GRADIENT_FULL_RGB = [0x8f, 0xf0, 0xa4];
const GRADIENT_MID_RGB = [0xf8, 0xe4, 0x5c];
const GRADIENT_LOW_RGB = [0xff, 0x7b, 0x63];
const INSTANCE_TAG_FIRST_RGB = [0x7e, 0xc8, 0xff];
const INSTANCE_TAG_LAST_RGB = [0xa8, 0x56, 0xe8];
const EMPTY_COLOR = '#ff0000';
const NEUTRAL_COLOR = '#9a9996';
// ClutterText ignores `foreground` on the first glyph (mutter#1324). A hair
// space before the first colored span makes 24h the second glyph so it paints.
const CLUTTER_MARKUP_PREFIX = '\u200A';

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
 * Resolve the poller's config file path honoring XDG_CONFIG_HOME.
 *
 * @returns {string} Absolute path to config.json.
 */
function configFilePath() {
  const configHome = GLib.getenv('XDG_CONFIG_HOME') ||
    GLib.build_filenamev([GLib.get_home_dir(), '.config']);
  return GLib.build_filenamev([configHome, 'agentic-cli-status', 'config.json']);
}

/**
 * Read the poller's config.json (best effort).
 *
 * Only consulted while status.json is missing; once the poller writes a
 * status file, its per-CLI `enabled` flags are authoritative.
 *
 * @returns {?object} Parsed config object, or null when unreadable.
 */
function readPollerConfig() {
  try {
    const [ok, bytes] = Gio.File.new_for_path(configFilePath())
      .load_contents(null);
    if (ok)
      return JSON.parse(new TextDecoder().decode(bytes));
  } catch (error) {
    log('debug', `config file unreadable: ${error}`);
  }
  return null;
}

/**
 * Resolve which CLIs may occupy a segment on the bar.
 *
 * @param {?object} config - Parsed config.json content, or null.
 * @param {object} defaults - Fallback enablement per CLI id.
 * @returns {object} Map of cli id to boolean enablement.
 */
function cliEnabledFromConfig(config, defaults) {
  const enabled = {};
  for (const cliId of Object.keys(defaults)) {
    const configured = config?.clis?.[cliId]?.enabled;
    enabled[cliId] = typeof configured === 'boolean'
      ? configured
      : defaults[cliId];
  }
  return enabled;
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
 * Map a Codex instance id onto the light-blue→purple identity gradient.
 *
 * ``CX1`` is light blue, the last registered id is purple, and ids in
 * between lerp the two. The compact bar uses this for the ``CXN`` tag
 * only; remaining-percent digits keep :func:`percentColor` (including red
 * when a window is exhausted).
 *
 * @param {number} id - 1-based instance id.
 * @param {number} count - How many instances are on the bar.
 * @returns {string} "#rrggbb" hex color.
 */
function instanceTagColor(id, count) {
  const n = Math.max(1, Number(count) || 1);
  const index = Math.max(1, Number(id) || 1);
  const t = n <= 1 ? 0 : Math.min(1, (index - 1) / (n - 1));
  const hex = INSTANCE_TAG_FIRST_RGB.map((start, i) => {
    const channel = Math.round(start + (INSTANCE_TAG_LAST_RGB[i] - start) * t);
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
 * Whether a snapshot's credit state is what is blocking the account.
 *
 * Codex reports two independent limit families per turn: the 5h/weekly
 * windows, and a pay-as-you-go credit balance. A depleted balance refuses
 * turns while every window still shows budget, so the window countdown alone
 * would claim the account is fine.
 *
 * @param {?object} credits - The `credits` object from a status snapshot.
 * @returns {boolean} True while a credit block is in force.
 */
function creditBlocked(credits) {
  return Boolean(credits && credits.blocked);
}

/**
 * Whether a reported credit balance amounts to nothing.
 *
 * Codex renders the balance as a string that may carry formatting, e.g. `"0"`,
 * `"0.00"` or `"$0.00"`, so the digits are parsed out rather than compared
 * textually.
 *
 * @param {*} balance - The `balance` field from a credit snapshot.
 * @returns {boolean} True when the balance is present and numerically zero.
 */
function zeroBalance(balance) {
  const digits = String(balance ?? '').replace(/[^0-9.-]/g, '');
  if (digits === '')
    return false;
  const amount = Number.parseFloat(digits);
  return Number.isFinite(amount) && amount === 0;
}

/**
 * Details-menu line describing a snapshot's credit state.
 *
 * @param {?object} credits - The `credits` object from a status snapshot.
 * @returns {string} Line text, or an empty string when there is nothing to say.
 */
function creditDetailText(credits) {
  if (!credits)
    return '';
  if (credits.unlimited === true)
    return 'credits: unlimited';
  const reached = String(credits.reached_type ?? '').trim();
  if (reached) {
    const phrase = reached === 'workspace_member_credits_depleted'
      ? 'team credits out'
      : 'credits out';
    return `credits: ${phrase}`;
  }
  const balance = credits.balance;
  if (balance === null || balance === undefined || balance === '')
    return '';
  // An account without a pay-as-you-go credit facility still reports a zero
  // balance. That is not "no credits left", it is "credits do not apply here",
  // so saying nothing beats a permanent `credits: 0 left` on the menu. A real
  // depletion still speaks through `reached_type` above, and an account that
  // does carry credits keeps reporting its balance even when it hits zero.
  if (credits.has_credits !== true && zeroBalance(balance))
    return '';
  return `credits: ${balance} left`;
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
 * @param {?string} [nameColor] - Override for the tag (Codex ``CXN`` identity).
 * @returns {string} Segment markup; a trailing "?" marks stale data.
 */
function segmentMarkup(tag, snapshot, nowEpoch = GLib.get_real_time() / 1000000, nameColor = null) {
  const windows = snapshot?.windows ?? [];
  // A credit block outlives any window reset, so it is flagged even when the
  // segment has no usable window numbers to color.
  const creditMark = creditBlocked(snapshot?.credits)
    ? `<span foreground="${EMPTY_COLOR}">$</span>`
    : '';
  if (!windows.length)
    return `<span foreground="${nameColor || NEUTRAL_COLOR}">${tag} –</span>${creditMark}`;
  if (snapshot.error) {
    const marker = snapshot.stale ? '?' : '!';
    return `<span foreground="${nameColor || NEUTRAL_COLOR}">${tag} –${marker}</span>${creditMark}`;
  }

  const weekly = windows.find(w => w.label === 'week');
  const shortTerm = windows.find(w => w.label === '5h' || w.label === 'rolling');
  const fiveHourExhausted = shortTerm && percentLeft(shortTerm, nowEpoch) === 0;
  const tagColor = fiveHourExhausted
    ? EMPTY_COLOR
    : percentColor(percentLeft(windows[0], nowEpoch));
  const labelColor = nameColor || tagColor;
  if (weekly && percentLeft(weekly, nowEpoch) === 0) {
    const staleMark = snapshot.stale ? '?' : '';
    return `<span foreground="${labelColor}">${tag} </span>` +
      `<span foreground="${EMPTY_COLOR}">` +
      `${resetClock(weekly.resets_at)}${staleMark}</span>${creditMark}`;
  }

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
    ? `<span foreground="${labelColor}">?</span>`
    : '';
  return `<span foreground="${labelColor}">${tag} </span>${parts}${staleMark}${creditMark}`;
}

/**
 * Whether a spending-page field is on-demand / spend-limit / pay-as-you-go.
 *
 * Those never belong on the CRS segment or its details menu.
 *
 * @param {string} text - Field label or value.
 * @returns {boolean} True when the text names an excluded surface.
 */
function isOnDemandField(text) {
  const n = String(text ?? '').toLowerCase().replace(/[^a-z0-9]/g, '');
  return n.includes('ondemand') || n.includes('spendlimit') ||
    n.includes('payasyougo');
}

/**
 * Human-readable included-usage pool name for the Cursor details menu.
 *
 * @param {?string} label - Snapshot window label.
 * @returns {string} Display name.
 */
/**
 * Extra CRS details-menu lines besides per-window remainders.
 *
 * Included-usage fields are listed; the snapshot-level usage-limit reset
 * date is never included (not ``usage limit reset — resets 14 Oct``).
 *
 * @param {object} snapshot - Cursor CLI entry from status.json.
 * @returns {string[]} Menu line texts without a leading indent.
 */
function cursorExtraDetailTexts(snapshot) {
  const lines = [];
  for (const field of snapshot?.included_fields ?? []) {
    const label = String(field?.label ?? '');
    const text = String(field?.text ?? '');
    if (!label || !text || isOnDemandField(label) || isOnDemandField(text))
      continue;
    if (/usage limit reset/i.test(label) || /usage limit reset/i.test(text))
      continue;
    lines.push(`${label}: ${text}`);
  }
  return lines;
}

function cursorWindowLabel(label) {
  switch (label) {
    case 'cursor_models':
      return 'Cursor Models';
    case 'other_models':
      return 'Other Models';
    case 'grok_bot':
      return 'Grok Bot';
    default:
      return String(label ?? '');
  }
}

/**
 * Compact CRS markup: the CRS tag and three included remainders.
 *
 * On-demand fields are never drawn. The snapshot-level usage-limit reset
 * date is not appended on the compact segment. Exhausted monthly pools
 * still show that window's own calendar date in place of 0; Grok Bot
 * (weekly) shows a weekday clock.
 *
 * @param {string} tag - Short tag, always ``CRS``.
 * @param {object} snapshot - Cursor CLI entry from status.json.
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {string} Segment markup.
 */
function cursorBarMarkup(tag, snapshot, nowEpoch = GLib.get_real_time() / 1000000) {
  const windows = snapshot?.windows ?? [];
  if (!windows.length || snapshot.error)
    return segmentMarkup(tag, snapshot, nowEpoch);
  const tagLeft = percentLeft(windows[0], nowEpoch);
  const tagColor = percentColor(tagLeft);
  const parts = windows.map(w => {
    const left = percentLeft(w, nowEpoch);
    if (left === 0) {
      const clock = w.label === 'grok_bot'
        ? resetClock(w.resets_at)
        : resetDate(w.resets_at);
      return `<span foreground="${EMPTY_COLOR}">${clock}</span>`;
    }
    return `<span foreground="${percentColor(left)}">${left}</span>`;
  }).join(`<span foreground="${NEUTRAL_COLOR}">·</span>`);
  const staleMark = snapshot.stale
    ? `<span foreground="${tagColor}">?</span>`
    : '';
  return `<span foreground="${tagColor}">${tag} </span>${parts}${staleMark}`;
}

/**
 * Join independent Codex account segments with a gray pipe.
 *
 * Agent Command Center registers extra ChatGPT logins as isolated
 * ``CODEX_HOME`` directories. Each account is tagged ``CX1``, ``CX2``, …
 * with a light-blue→purple identity color. Remaining-percent digits keep
 * the usual green→red usage gradient, including a red reset clock next to
 * the tag when a window is exhausted.
 *
 * @param {string} tag - Unused; kept so the call site matches other CLIs.
 * @param {object} snapshot - Codex CLI entry from status.json.
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {string} Combined segment markup.
 */
function codexBarMarkup(tag, snapshot, nowEpoch = GLib.get_real_time() / 1000000) {
  const instances = snapshot?.instances;
  if (!Array.isArray(instances) || instances.length < 1) {
    const id = snapshot?.selected || 1;
    return segmentMarkup(
      `CX${id}`,
      snapshot,
      nowEpoch,
      instanceTagColor(id, 1),
    );
  }
  const count = instances.length;
  const sep = `<span foreground="${NEUTRAL_COLOR}"> | </span>`;
  return instances.map((instance) => {
    const id = instance?.id ?? 1;
    return segmentMarkup(
      `CX${id}`,
      instance,
      nowEpoch,
      instanceTagColor(id, count),
    );
  }).join(sep);
}

/**
 * Clamp a forecast probability to an integer 0-100, or null when missing.
 *
 * @param {*} value - Raw consensus percentage from status.json.
 * @returns {?number} Rounded percent, or null when the side is absent.
 */
function roundedForecastPercent(value) {
  if (typeof value !== 'number' || !Number.isFinite(value))
    return null;
  return Math.round(Math.min(100, Math.max(0, value)));
}

/**
 * Read the 24h and 48h consensus values without inventing a missing side.
 *
 * Prefers explicit `consensus_percent_24h` / `consensus_percent_48h`. Falls
 * back to `consensus_percent` only for the configured `horizon` so a snapshot
 * that still publishes a single number keeps working.
 *
 * @param {?object} forecast - The `codex_reset_forecast` status entry.
 * @returns {{p24: ?number, p48: ?number}} Rounded percents; null means omit.
 */
function forecastHorizonPercents(forecast) {
  let p24 = roundedForecastPercent(forecast?.consensus_percent_24h);
  let p48 = roundedForecastPercent(forecast?.consensus_percent_48h);
  const configured = roundedForecastPercent(forecast?.consensus_percent);
  const horizon = String(forecast?.horizon ?? '24h');
  if (p24 === null && horizon !== '48h')
    p24 = configured;
  if (p48 === null && horizon === '48h')
    p48 = configured;
  return {p24, p48};
}

/**
 * Visible 24-48h consensus text: `26%-48%`, or one side when the other is absent.
 *
 * @param {?number} p24 - Rounded 24h consensus, or null.
 * @param {?number} p48 - Rounded 48h consensus, or null.
 * @returns {string} Compact pair (or a single `NN%`); empty when both missing.
 */
function formatForecastConsensusPair(p24, p48) {
  if (p24 !== null && p48 !== null)
    return `${p24}%-${p48}%`;
  if (p24 !== null)
    return `${p24}%`;
  if (p48 !== null)
    return `${p48}%`;
  return '';
}

/**
 * Color one forecast percent with the shared remaining-budget scale.
 *
 * 100% is green (`#8ff0a4`), 0% is red (`#ff0000`) — the same
 * :func:`percentColor` mapper used by every other bar percentage. There is
 * no forecast-specific gradient.
 *
 * @param {number} percent - Rounded 0-100 consensus.
 * @returns {string} Pango span for `NN%`.
 */
function forecastPercentMarkup(percent) {
  return `<span foreground="${percentColor(percent)}">${percent}%</span>`;
}

/**
 * Pango 24-48h consensus: each side uses percentColor; the hyphen is neutral.
 *
 * @param {?number} p24 - Rounded 24h consensus, or null.
 * @param {?number} p48 - Rounded 48h consensus, or null.
 * @returns {string} Colored pair markup, or empty when both sides are missing.
 */
function formatForecastConsensusMarkup(p24, p48) {
  if (p24 !== null && p48 !== null)
    return `${forecastPercentMarkup(p24)}` +
      `<span foreground="${NEUTRAL_COLOR}">-</span>` +
      `${forecastPercentMarkup(p48)}`;
  if (p24 !== null)
    return forecastPercentMarkup(p24);
  if (p48 !== null)
    return forecastPercentMarkup(p48);
  return '';
}

/**
 * Format the consensus probability of a discretionary Codex limit reset.
 *
 * When both 24h and 48h values exist the compact bar shows `26%-48%`. A
 * missing side is omitted rather than filled with a made-up number.
 *
 * @param {?object} forecast - The `codex_reset_forecast` status entry.
 * @returns {string} Pango markup for the percentage, or an empty string when
 *   forecasting is disabled or no consensus is available.
 */
function codexResetForecastMarkup(forecast) {
  if (forecast?.enabled !== true)
    return '';
  const {p24, p48} = forecastHorizonPercents(forecast);
  const markup = formatForecastConsensusMarkup(p24, p48);
  if (!markup)
    return '';
  log('debug', `forecast compact consensus ${formatForecastConsensusPair(p24, p48)}`);
  const staleMark = forecast.stale
    ? `<span foreground="${NEUTRAL_COLOR}">?</span>`
    : '';
  // ClutterText ignores `foreground` on the first glyph (mutter#1324). A
  // wrapping span is not enough: the 24h digit would still start at index 0
  // and stay the theme color while 48h painted. Prefix a hair-space glyph so
  // percentColor applies to 24h.
  return `${CLUTTER_MARKUP_PREFIX}<span>${markup}${staleMark}</span>`;
}

/**
 * Put the reset forecast immediately left of all enabled Codex instances.
 *
 * The caller only invokes this helper for an enabled Codex snapshot, so the
 * forecast can never occupy the bar by itself when Codex monitoring is off.
 *
 * @param {?object} forecast - The `codex_reset_forecast` status entry.
 * @param {string} tag - Compact Codex tag.
 * @param {object} snapshot - Codex CLI status entry.
 * @param {number} [nowEpoch] - Current epoch seconds; injectable for tests.
 * @returns {string} Combined forecast and Codex Pango markup.
 */
function codexBarWithForecastMarkup(
  forecast,
  tag,
  snapshot,
  nowEpoch = GLib.get_real_time() / 1000000,
) {
  const codex = codexBarMarkup(tag, snapshot, nowEpoch);
  const resetForecast = codexResetForecastMarkup(forecast);
  if (!resetForecast)
    return codex;
  return `${resetForecast}<span foreground="${NEUTRAL_COLOR}"> | </span>${codex}`;
}

/** Format a forecast probability for the details menu. */
function formatForecastPercent(value) {
  if (typeof value !== 'number' || !Number.isFinite(value))
    return '';
  const rounded = Math.round(Math.min(100, Math.max(0, value)) * 10) / 10;
  return `${rounded}%`;
}

/** Format a published cumulative trading volume as whole US dollars. */
function formatForecastVolume(value) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0)
    return '';
  return `$${value.toLocaleString('en-US', {
    maximumFractionDigits: 0,
  })}`;
}

/** Shorten a Polymarket expiry such as `by September 22` to `22.9`. */
function formatPolymarketDate(label) {
  const withoutBy = label.replace(/^by\s+/i, '');
  const match = /^([A-Za-z]+) (\d{1,2})$/.exec(withoutBy);
  if (!match)
    return withoutBy;
  const months = ['January', 'February', 'March', 'April', 'May', 'June',
    'July', 'August', 'September', 'October', 'November', 'December'];
  const month = months.indexOf(match[1]) + 1;
  const day = Number(match[2]);
  return month > 0 && day >= 1 && day <= 31 ? `${day}.${month}` : withoutBy;
}

/**
 * Build the reset-forecast entries shown above every CLI details section.
 *
 * Each source gets one named row containing every horizon it publishes. A
 * failed source remains visible as unavailable so the menu does not silently
 * make a multi-source consensus look healthier than it is.
 *
 * @param {?object} forecast - The `codex_reset_forecast` status entry.
 * @returns {object[]} Heading followed by one linked entry per source.
 */
function codexResetForecastMenuEntries(forecast) {
  if (forecast?.enabled !== true)
    return [];
  const horizon = String(forecast.horizon ?? '24h');
  const {p24, p48} = forecastHorizonPercents(forecast);
  const pair = formatForecastConsensusPair(p24, p48);
  let heading;
  if (p24 !== null && p48 !== null)
    heading = `Codex reset forecast — 24-48h consensus: ${pair}`;
  else if (pair)
    heading = `Codex reset forecast — ${p24 === null ? '48h' : horizon} consensus: ${pair}`;
  else
    heading = `Codex reset forecast — ${horizon} consensus unavailable`;
  log('debug', `forecast menu heading ${heading}`);
  const entries = [{
    kind: 'heading',
    name: 'Codex reset forecast',
    text: heading,
    url: null,
  }];
  for (const site of forecast.sites ?? []) {
    const name = String(site?.name ?? site?.id ?? 'Unknown site').trim() ||
      'Unknown site';
    const predictions = (site?.predictions ?? []).flatMap(prediction => {
      const percent = formatForecastPercent(prediction?.percent);
      if (!percent)
        return [];
      const rawLabel = String(prediction?.label ?? horizon).trim() || horizon;
      const label = site?.id === 'polymarket'
        ? formatPolymarketDate(rawLabel) : rawLabel;
      const volume = formatForecastVolume(prediction?.volume_usd);
      return [`${label}: ${percent}${volume ? ` ${volume}` : ''}`];
    });
    entries.push({
      kind: 'site',
      name,
      text: predictions.length
        ? `${name} — ${predictions.join(' · ')}`
        : `${name} — unavailable`,
      url: site?.url ?? null,
    });
  }
  return entries;
}

/**
 * Open a vetted forecast site in the desktop's default browser.
 *
 * Only HTTPS links are accepted because status.json is a local data boundary
 * and must not be able to launch arbitrary URI handlers.
 *
 * @param {*} url - Candidate site URL from status.json.
 * @param {string} name - Site name used in diagnostics.
 * @returns {boolean} Whether an asynchronous browser launch was requested.
 */
function openExternalUrl(url, name) {
  if (typeof url !== 'string' || !url.startsWith('https://')) {
    log('warn', `refusing non-HTTPS forecast link for ${name}`);
    return false;
  }
  try {
    Gio.AppInfo.launch_default_for_uri_async(
      url,
      null,
      null,
      (_source, result) => {
        try {
          Gio.AppInfo.launch_default_for_uri_finish(result);
          log('info', `opened forecast site ${name}`);
        } catch (error) {
          log('warn', `could not open forecast site ${name}: ${error}`);
        }
      },
    );
    return true;
  } catch (error) {
    log('warn', `could not request forecast site ${name}: ${error}`);
    return false;
  }
}

/**
 * Details-menu heading for one Codex account instance.
 *
 * @param {object} instance - One entry from snapshot.instances.
 * @returns {string} Heading such as "Codex [2] trial *".
 */
function codexInstanceTitle(instance) {
  const id = instance?.id ?? '?';
  const label = String(instance?.label ?? '').trim();
  const mark = instance?.selected ? ' *' : '';
  return label ? `Codex [${id}] ${label}${mark}` : `Codex [${id}]${mark}`;
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

/**
 * Details-menu heading for one CLI or Codex account section.
 *
 * @param {string} name - Human-readable CLI or instance title.
 * @param {object} snapshot - One CLI or instance entry from status.json.
 * @returns {string} Heading such as "Codex [1] (team) — 2 resets available".
 */
function cliSectionHeading(name, snapshot) {
  const plan = snapshot.error ? '' : (snapshot.plan ? ` (${snapshot.plan})` : '');
  return `${name}${plan}${resetAvailabilityText(snapshot.resets_available)}`;
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
      // No status file yet: fall back to the poller's own config so opt-in
      // CLIs (OpenRouter, OpenCode Go) do not flash a placeholder segment
      // the user never asked for.
      const enabled = cliEnabledFromConfig(readPollerConfig(), DEFAULT_CLI_ENABLED);
      for (const [cliId, tag] of CLI_TAGS) {
        const label = this._segments.get(cliId);
        if (enabled[cliId] === false) {
          label.visible = false;
          continue;
        }
        label.text = `${tag} –`;
        label.style_class = 'acs-segment acs-off';
        label.visible = true;
      }
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        'Waiting for the agentic-cli-status poller…', { reactive: false }));
      return;
    }
    const codexSnapshot = status.clis?.codex;
    if (codexSnapshot && codexSnapshot.enabled !== false)
      this._appendForecastSection(status.codex_reset_forecast);
    const names = {
      codex: 'Codex',
      claude_code: 'Claude Code',
      grok: 'Grok CLI',
      opencode_go: 'OpenCode Go',
      openrouter: 'OpenRouter',
      cursor: 'Cursor',
    };
    for (const [cliId, tag] of CLI_TAGS) {
      const snapshot = status.clis?.[cliId];
      const label = this._segments.get(cliId);
      if (!snapshot || snapshot.enabled === false) {
        label.visible = false;
        continue;
      }
      label.visible = true;
      const markup = cliId === 'codex'
        ? codexBarWithForecastMarkup(status.codex_reset_forecast, tag, snapshot)
        : cliId === 'cursor'
          ? cursorBarMarkup(tag, snapshot)
          : segmentMarkup(tag, snapshot);
      label.clutter_text.set_markup(markup);
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
   * Append the reset consensus and every named source at the top of the menu.
   *
   * @param {?object} forecast - The `codex_reset_forecast` status entry.
   * @returns {void}
   */
  _appendForecastSection(forecast) {
    const entries = codexResetForecastMenuEntries(forecast);
    for (const entry of entries) {
      if (entry.kind === 'heading') {
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
          entry.text, { reactive: false }));
        continue;
      }
      const item = new PopupMenu.PopupBaseMenuItem({
        reactive: false,
        can_focus: false,
      });
      const row = new St.BoxLayout({
        y_align: Clutter.ActorAlign.CENTER,
      });
      const hasUrl = typeof entry.url === 'string' &&
        entry.url.startsWith('https://');
      const button = new St.Button({
        label: '↗',
        style_class: 'acs-forecast-link-button',
        reactive: hasUrl,
        can_focus: hasUrl,
        track_hover: hasUrl,
        accessible_name: `Open ${entry.name} in browser`,
      });
      button.connect('clicked', () => {
        this.menu.close();
        openExternalUrl(entry.url, entry.name);
      });
      row.add_child(button);
      row.add_child(new St.Label({
        text: entry.text,
        y_align: Clutter.ActorAlign.CENTER,
      }));
      item.add_child(row);
      this.menu.addMenuItem(item);
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
    this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
      cliSectionHeading(name, snapshot), { reactive: false }));
    if (snapshot.error) {
      this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
        `   ⚠ ${snapshot.error}`, { reactive: false }));
      return;
    }
    const nowEpoch = GLib.get_real_time() / 1000000;
    const creditLine = creditDetailText(snapshot.credits);
    if (creditLine) {
      const item = new PopupMenu.PopupMenuItem('', { reactive: false });
      const color = creditBlocked(snapshot.credits) ? EMPTY_COLOR : NEUTRAL_COLOR;
      item.label.clutter_text.set_markup(
        `   <span foreground="${color}">` +
        `${GLib.markup_escape_text(creditLine, -1)}</span>`);
      this.menu.addMenuItem(item);
    }
    for (const window of snapshot.windows ?? []) {
      const left = percentLeft(window, nowEpoch);
      // A window that has already rolled over reads as full; its old reset
      // time is history, so don't dangle it as if still upcoming.
      const monthLike = window.label === 'month' ||
        window.label === 'cursor_models' || window.label === 'other_models';
      const reset = resetPassed(window, nowEpoch)
        ? '' : resetsText(window.resets_at, monthLike ? 'month' : window.label);
      const title = name === 'Cursor'
        ? cursorWindowLabel(window.label)
        : String(window.label ?? '');
      const item = new PopupMenu.PopupMenuItem('', { reactive: false });
      item.label.clutter_text.set_markup(
        `   ${GLib.markup_escape_text(title, -1)}: ` +
        `<span foreground="${percentColor(left)}">${left}% left</span>` +
        GLib.markup_escape_text(reset, -1));
      this.menu.addMenuItem(item);
    }
    if (name === 'Cursor') {
      for (const line of cursorExtraDetailTexts(snapshot)) {
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
          `   ${line}`, { reactive: false }));
      }
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
