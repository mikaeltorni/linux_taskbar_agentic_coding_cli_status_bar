/* extension.js — acc-model-status@local
 *
 * GNOME Shell taskbar indicator for Agent Command Center and independent Harbor runs. It renders
 * one running-agent amount to the left of each provider icon (Claude, Codex,
 * Grok, Qwen, …), with a compact tok/s chip on the right when that harness is
 * producing tokens.
 *
 * Separation of concerns (same contract as agentic-cli-status@local):
 *   - Agent Command Center's monitor OWNS the data: it publishes the document
 *     `model_status_bar.json` into the tracker state directory.
 *   - This extension joins that document to GNOME Shell's live window metadata
 *     so only agents with an open terminal window are rendered, across every
 *     workspace. It never scans tmux or talks to the network; it watches the
 *     file and Shell window/workspace signals directly. Activating an agent
 *     row launches ACC's shared window selector for that exact note id.
 *
 * Components:
 *   AccModelStatusButton  panel button: segment box + agent list menu
 *   payloadFile()         resolves the published document's path
 *   readPayload()         parses/validates the document, tolerating partial writes
 */

import {attachHarborStatus, harborCountText, formatHarborEstimate} from './harbor.js';
import Cairo from 'cairo';
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import Pango from 'gi://Pango';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as BoxPointer from 'resource:///org/gnome/shell/ui/boxpointer.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

/** Document published by the Agent Command Center monitor. */
const PAYLOAD_FILENAME = 'model_status_bar.json';
/** Local acknowledgement state for finished turns. */
const UNREAD_FILENAME = 'model-status-unread.json';
/** Tracker state directory relative to the user's state home. */
const STATE_DIR_NAME = 'codex-agent-tracker';
/** Environment override honored by the tracker (and therefore by us). */
const STATE_DIR_ENV = 'CODEX_AGENT_TRACKER_STATE_DIR';
/** Mutter key names that can alias the physical key above Tab. */
const AGENT_STATUS_MENU_ACCELERATORS = new Set(['<alt>above_tab', '<alt>section', '<alt>grave', '<alt>0x31']);
/** Preserve the user's group-switch accelerators while this extension is active. */
const SWITCH_GROUP_SCHEMA = 'org.gnome.desktop.wm.keybindings';
const SWITCH_GROUP_KEY = 'switch-group';
const MENU_ACCELERATOR_BACKUP_FILENAME = 'model-status-menu-keybinding-backup.json';
/** Safety-net re-read interval (seconds); the file monitor does the real work. */
const POLL_SECONDS = 15;
/** Payload older than this is rendered dimmed — the monitor is not publishing. */
const STALE_SECONDS = 180;
/** Fallback icon for providers this extension ships no artwork for. */
const FALLBACK_ICON = 'acc-provider-generic-symbolic';
/** Phases contributing to the scoped provider counters. */
const RUNNING_PHASES = new Set(['RUNNING']);
const ATTENTION_PHASES = new Set(['WAITING', 'INTERRUPTED']);
/** Keep the CX identity endpoints in sync with agentic-cli-status@local. */
const INSTANCE_TAG_FIRST_RGB = [0x7e, 0xc8, 0xff];
const INSTANCE_TAG_LAST_RGB = [0xa8, 0x56, 0xe8];
/**
 * Agent phase palette — the SAME colors the phrase automation notes menu uses
 * for its agent badges (AGENT_PHASE_BADGE_COLORS in
 * src/overlay/notes_display.py). Keep the two in sync: a status looks the same
 * whether you read it in the notes overlay or in this panel menu.
 */
const PHASE_COLORS = {
    RUNNING: '#e6c200',
    GOAL: '#a855f7',
    QUESTION: '#c084fc',
    ANSWERED: '#5ca8ff',
    RATE: '#f472b6',
    ATTN: '#ffb454',
    WAITING: '#ffb454',
    DONE: '#22c55e',
    DRAFT: '#84c32f',
    QUEUED_RESET: '#b5d747',
    STOPPED: '#ef4444',
    INTERRUPTED: '#ef4444',
    TRANSFERRED: '#c45c5c',
    UNKNOWN: '#7f8ea3',
};

/**
 * Color for one agent phase.
 *
 * @param {string} phase - phase key from the payload (any case).
 * @returns {string} CSS color; unknown phases render in the muted grey the
 *     notes menu uses for UNKNOWN.
 */
function phaseColor(phase) {
    const key = String(phase || '').trim().toUpperCase();
    return PHASE_COLORS[key] || PHASE_COLORS.UNKNOWN;
}

/**
 * Compact ACC run clock: ``M:SS`` below one hour, else ``H:MM:SS``.
 *
 * Same helper the top-right ActivityAgeBanner uses (``format_run_timer``), so
 * the click-menu badge and the terminal frame never disagree on elapsed text.
 *
 * @param {number} seconds - elapsed seconds since ``run_started_at``.
 * @returns {string} compact clock text.
 */
function formatRunTimer(seconds) {
    const elapsed = Math.max(Math.floor(Number(seconds) || 0), 0);
    const hours = Math.floor(elapsed / 3600);
    const minutes = Math.floor((elapsed % 3600) / 60);
    const secs = elapsed % 60;
    if (hours)
        return `${hours}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
    return `${minutes}:${String(secs).padStart(2, '0')}`;
}

/**
 * Visible badge text for one click-menu agent row.
 *
 * A queued auto-continue shows a negative reset clock; a live run with a known
 * start shows the elapsed clock. Measured workflow counts precede the clock
 * in brackets without a phase name. Other phases keep their phase word.
 *
 * @param {object} agent - payload agent row.
 * @param {number} [nowMs] - clock override in milliseconds (tests).
 * @returns {string} clock text or uppercase phase word.
 */
/**
 * Use DONE for finished work. RATE stays only on instances still waiting to start.
 * Parameters: agent - published row.
 * Returns: the phase word shown on the taskbar.
 */
function displayedPhase(agent) {
    let phase = String(agent && agent.phase || 'UNKNOWN').trim().toUpperCase();
    if (phase === 'RATE' && taskMarkedDone(agent))
        phase = 'DONE';
    return phase;
}

/** Format the task's clock and measured counts.
 * Parameters: agent - scoped payload row; nowMs - optional clock epoch.
 * Returns: task clock and phase.
 */
function agentPhaseBadgeText(agent, nowMs) {
    print(`agent=${agent} nowMs=${nowMs}`);
    const phase = displayedPhase(agent);
    const completed = agent?.workflow?.phase_completed;
    const total = agent?.workflow?.phase_total;
    const showProgress = phase !== 'DONE' && !agent?.unread_completion &&
        Number.isInteger(completed) && Number.isInteger(total) && total > 0 && completed >= 0 && completed <= total;
    const prefix = showProgress ? `[${completed}/${total}] ` : '';
    const now = Number.isFinite(nowMs) ? nowMs : Date.now();
    const resetAt = Date.parse(agent?.queued_reset_at || '');
    const started = Date.parse(agent?.run_started_at || '');
    let result = `${prefix}${phase}`;
    if (phase !== 'DONE' && resetAt > now)
        result = `${prefix}-${formatRunTimer(Math.ceil((resetAt - now) / 1000))}`;
    else if (phase === 'RUNNING' && agent?.run_active && !Number.isNaN(started))
        result = `${prefix}${formatRunTimer((now - started) / 1000)}`;
    print(result);
    return result;
}

/**
 * Color for one click-menu badge, retaining interruption red over retry clocks.
 * Parameters: agent - payload row; nowMs - optional reference clock.
 * Returns: CSS color from the phase palette.
 */
function agentPhaseBadgeColor(agent, nowMs) {
    print(`agent=${agent} nowMs=${nowMs}`);
    const now = Number.isFinite(nowMs) ? nowMs : Date.now();
    const phase = displayedPhase(agent);
    let result = phaseColor(phase);
    if (phase !== 'INTERRUPTED' && phase !== 'DONE') {
        if (agent && agent.queued_reset_at && Date.parse(agent.queued_reset_at) > now)
            result = PHASE_COLORS.QUEUED_RESET;
        else if (phase === 'RUNNING' && agent && agent.goal_active && agent.run_active &&
            !Number.isNaN(Date.parse(agent.run_started_at || '')))
            result = PHASE_COLORS.GOAL;
    }
    if (agent?.workflow?.interaction?.mode === 'question')
        result = PHASE_COLORS[agent.workflow.interaction.state === 'complete' ? 'ANSWERED' : 'QUESTION'];
    print(result);
    return result;
}

/**
 * Resolve the published payload path.
 *
 * Honors CODEX_AGENT_TRACKER_STATE_DIR so a test or a relocated tracker state
 * directory is picked up without editing the extension.
 *
 * @returns {Gio.File} the model_status_bar.json location (may not exist yet).
 */
function payloadFile() {
    const override = GLib.getenv(STATE_DIR_ENV);
    const dir = override && override.length > 0
        ? override
        : GLib.build_filenamev([GLib.get_user_state_dir(), STATE_DIR_NAME]);
    return Gio.File.new_for_path(GLib.build_filenamev([dir, PAYLOAD_FILENAME]));
}

/**
 * Read and validate the published document.
 *
 * Parameters: file - payload location.
 * Returns: normalized payload, or null when absent/unreadable/
 *     mid-write (the monitor writes atomically, but a truncated read still
 *     must never throw into the Shell).
 */
function readPayload(file) {
    let bytes;
    try {
        const [ok, contents] = file.load_contents(null);
        if (!ok) {
            return null;
        }
        bytes = contents;
    } catch (_error) {
        // Missing file is the normal "ACC not running yet" case.
        return null;
    }

    let parsed;
    try {
        parsed = JSON.parse(new TextDecoder().decode(bytes));
    } catch (error) {
        logError(error, 'acc-model-status: unparsable payload');
        return null;
    }
    if (!parsed || typeof parsed !== 'object') {
        return null;
    }

    const providers = Array.isArray(parsed.providers) ? parsed.providers : [];
    const harnessRates = Array.isArray(parsed.harness_rates) ? parsed.harness_rates : [];
    const result = {
        providers: providers.filter(entry => entry && typeof entry === 'object'),
        harnessRates: harnessRates.filter(entry => entry && typeof entry === 'object'),
        totalLive: Number(parsed.total_live) || 0,
        totalRunning: Number(parsed.total_running) || 0,
        codexInstanceCount: Math.max(1, Number(parsed.codex_instance_count) || 1),
        announcementsMuted: parsed.announcements_muted === true,
        updatedAt: typeof parsed.updated_at === 'string' ? parsed.updated_at : '',
    };
    return result;
}

/**
 * Load unread completion decisions from the tracker state directory.
 *
 * @param {Gio.File} file - local state file next to the published payload.
 * @returns {Map<string, object>} prior phase and acknowledgement by session.
 */
function readUnreadStates(file) {
    try {
        if (!file.query_exists(null))
            return new Map();
        const [ok, bytes] = file.load_contents(null);
        if (!ok)
            return new Map();
        const rows = JSON.parse(new TextDecoder().decode(bytes));
        if (!Array.isArray(rows))
            return new Map();
        return new Map(rows.filter(row => Array.isArray(row) && row.length === 2 &&
            typeof row[0] === 'string' && row[1] && typeof row[1] === 'object'));
    } catch (error) {
        logError(error, 'acc-model-status: unread completion state unavailable');
        return new Map();
    }
}

/**
 * Save completion decisions only when a phase or acknowledgement changes.
 *
 * @param {Gio.File} file - local state file next to the published payload.
 * @param {Map<string, object>} states - current decisions by session.
 */
function saveUnreadStates(file, states) {
    try {
        GLib.mkdir_with_parents(file.get_parent().get_path(), 0o700);
        GLib.file_set_contents(file.get_path(), JSON.stringify([...states]));
        GLib.chmod(file.get_path(), 0o600);
    } catch (error) {
        logError(error, 'acc-model-status: could not save unread completion state');
    }
}

/** Return existing group-switch values that collide with Alt+Above_Tab. */
function agentStatusMenuShortcutBindings(bindings) {
    return (Array.isArray(bindings) ? bindings : []).filter(binding =>
        typeof binding === 'string' && AGENT_STATUS_MENU_ACCELERATORS.has(binding.toLowerCase()));
}

/** Remove layout aliases for the accelerator from GNOME's group-switch list. */
function withoutAgentStatusMenuShortcuts(bindings) {
    return (Array.isArray(bindings) ? bindings : []).filter(binding =>
        typeof binding !== 'string' || !AGENT_STATUS_MENU_ACCELERATORS.has(binding.toLowerCase()));
}

/** Restore saved aliases without replacing unrelated group-switch changes. */
function restoreAgentStatusMenuShortcuts(bindings, shortcuts) {
    const restored = Array.isArray(bindings) ? [...bindings] : [];
    const existing = new Set(restored.map(binding => String(binding).toLowerCase()));
    for (const shortcut of agentStatusMenuShortcutBindings(shortcuts)) {
        const normalized = shortcut.toLowerCase();
        if (!existing.has(normalized)) {
            restored.push(shortcut);
            existing.add(normalized);
        }
    }
    return restored;
}

/** Read the extension's persisted list of group-switch aliases to restore. */
function readMenuShortcutBackup(file) {
    if (!file.query_exists(null))
        return [];
    try {
        const [ok, bytes] = file.load_contents(null);
        if (!ok)
            throw new Error('backup file could not be read');
        const parsed = JSON.parse(new TextDecoder().decode(bytes));
        if (!Array.isArray(parsed) || parsed.some(binding => typeof binding !== 'string'))
            throw new Error('backup file has an invalid shape');
        return agentStatusMenuShortcutBindings(parsed);
    } catch (error) {
        logError(error, 'acc-model-status: could not read menu shortcut backup');
        return null;
    }
}

/** Persist only the user's colliding accelerators so Shell restarts can recover. */
function saveMenuShortcutBackup(file, shortcuts) {
    try {
        GLib.mkdir_with_parents(file.get_parent().get_path(), 0o700);
        GLib.file_set_contents(file.get_path(), JSON.stringify(shortcuts));
        GLib.chmod(file.get_path(), 0o600);
        return true;
    } catch (error) {
        logError(error, 'acc-model-status: could not save menu shortcut backup');
        return false;
    }
}

/** Remove the persisted shortcut backup after restoring the user's setting. */
function deleteMenuShortcutBackup(file) {
    try {
        if (file.query_exists(null))
            file.delete(null);
    } catch (error) {
        logError(error, 'acc-model-status: could not remove menu shortcut backup');
    }
}

/** Return the same blue-to-purple CX identity color as the rate-limit bar. */
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

/** Return a numbered menu identity label and color when the agent kind is known. */
function codexInstanceLabel(agent, count) {
    const kind = String(agent?.kind || '').toLowerCase();
    const tags = {
        api: 'AP',
        claude: 'CC',
        cline: 'CL',
        codex: 'CX',
        grok: 'GK',
        nvidia: 'NA',
        opencode: 'OC',
        openrouter: 'OA',
        qwen: 'QA',
    };
    const prefix = tags[kind] || '';
    if (!prefix)
        return null;
    let id = Number(agent?.codex_instance);
    if (!Number.isInteger(id) || id < 1) {
        if (kind === 'codex')
            return null;
        id = 1;
    }
    return [`${prefix}${id}`, instanceTagColor(id, count)];
}

/**
 * Format one harness rate row. Missing counters render as ``n/a``.
 *
 * @param {object} row - published harness_rates entry.
 * @returns {string} one menu line, e.g. ``Codex 12/s  80/min  900/h``.
 */
function formatCompactTokens(value) {
    if (value === null || value === undefined)
        return 'n/a';
    const number = Number(value);
    if (!Number.isFinite(number) || number < 0)
        return 'n/a';
    const amount = Math.trunc(number);
    if (amount < 1000)
        return String(amount);
    const suffixes = ['k', 'M', 'B', 'T'];
    let scaled = amount;
    let index = -1;
    while (scaled >= 1000 && index < suffixes.length - 1) {
        scaled /= 1000;
        index += 1;
    }
    const text = index === 0 || scaled >= 10 ? scaled.toFixed(0) : scaled.toFixed(1).replace(/\.0$/, '');
    return `${text}${suffixes[index]}`;
}

function formatHarnessRateText(row) {
    const label = String(row?.label || row?.kind || '?');
    if (row?.tokens_per_second === null || row?.tokens_per_second === undefined)
        return `${label} n/a`;
    return `${label} ${formatCompactTokens(row.tokens_per_second)}/s  ${formatCompactTokens(row.tokens_per_minute)}/min  ${formatCompactTokens(row.tokens_per_hour)}/h`;
}

/**
 * True when the harness reported tokens in the last minute.
 *
 * @param {object} row - published harness_rates entry.
 * @returns {boolean} whether the menu should list this harness.
 */
function harnessHasMinuteActivity(row) {
    const value = Number(row?.tokens_per_minute);
    return Number.isFinite(value) && value > 0;
}

/**
 * Return menu lines for harnesses with tokens-per-minute activity.
 *
 * Idle 0 tok/s rows and missing counters are omitted. A harness that
 * finished a turn in the last minute still appears (tok/s may be 0).
 *
 * @param {object} payload - normalized payload from {@link readPayload}.
 * @returns {string[]} one line per active harness, in payload order.
 */
function harnessRateLines(payload) {
    const rows = Array.isArray(payload?.harnessRates) ? payload.harnessRates : [];
    return rows.filter(harnessHasMinuteActivity).map(formatHarnessRateText);
}

/**
 * Compact panel rate: live tok/s, or tok/min when the 1s window is 0.
 *
 * @param {object} row - published harness_rates entry.
 * @returns {string} ``12/s``, ``80/m``, or ``''`` when idle.
 */
function formatHarnessPanelRateText(row) {
    if (!harnessHasMinuteActivity(row))
        return '';
    const perSecond = Number(row?.tokens_per_second);
    if (Number.isFinite(perSecond) && perSecond > 0)
        return `${formatCompactTokens(perSecond)}/s`;
    const perMinute = Number(row?.tokens_per_minute);
    if (Number.isFinite(perMinute) && perMinute > 0)
        return `${formatCompactTokens(Math.max(1, Math.round(perMinute / 60)))}/s`;
    return '';
}

/**
 * Return minute-active harness rows in payload order.
 *
 * @param {object} payload - normalized payload from {@link readPayload}.
 * @returns {object[]} published rate rows with tokens in the last minute.
 */
function panelHarnessRateRows(payload) {
    const rows = Array.isArray(payload?.harnessRates) ? payload.harnessRates : [];
    return rows.filter(harnessHasMinuteActivity);
}

/**
 * Age of an ISO-8601 timestamp in seconds.
 *
 * @param {string} iso - timestamp from the payload.
 * @returns {number} seconds since publication, or 0 when unparsable (an
 *     unreadable timestamp must not make a fresh payload look stale).
 */
function ageSeconds(iso) {
    if (!iso)
        return 0;
    const stamp = Date.parse(iso);
    if (Number.isNaN(stamp))
        return 0;
    return Math.max(0, (Date.now() - stamp) / 1000);
}

/**
 * Normalize user-visible identity text for stable title matching.
 *
 * @param {*} value - payload field or Shell window title.
 * @returns {string} case-folded text with whitespace collapsed.
 */
function normalizedIdentity(value) {
    return String(value || '').trim().replace(/\s+/g, ' ').toLocaleLowerCase();
}

/**
 * Return how strongly one ACC agent row matches one terminal window title.
 *
 * ACC owns Kitty titles in the form ``description - project - worktree``.
 * The payload carries that same summarized description, plus session/tmux
 * identities. Exact identity markers win; the normal title prefix is the
 * fallback. Placeholder rows such as ``(codex)`` intentionally do not match.
 *
 * @param {object} agent - agent metadata from the ACC payload.
 * @param {string} title - title read from Meta.Window.
 * @returns {number} positive match score, or zero for no match.
 */
function agentWindowMatchScore(agent, title) {
    const windowTitle = normalizedIdentity(title);
    if (!windowTitle)
        return 0;

    const sessionId = normalizedIdentity(agent.session_id);
    if (sessionId && windowTitle.includes(sessionId))
        return 30000 + sessionId.length;
    const tmuxName = normalizedIdentity(agent.tmux_name);
    if (tmuxName && windowTitle.includes(tmuxName))
        return 20000 + tmuxName.length;

    const agentTitle = normalizedIdentity(agent.title);
    if (!agentTitle || /^\([^)]+\)$/.test(agentTitle))
        return 0;
    if (windowTitle !== agentTitle && !windowTitle.startsWith(`${agentTitle} - `))
        return 0;

    let score = 10000 + agentTitle.length;
    const worktree = String(agent.worktree || '').replace(/\/+$/, '');
    const projectSlug = normalizedIdentity(worktree.split('/').pop());
    if (projectSlug && windowTitle.includes(` - ${projectSlug} - `))
        score += 1000;
    return score;
}

/** Return a numeric monitor sort key, placing unknown monitors last. */
function monitorSortValue(monitor) {
    if (monitor === null || monitor === undefined)
        return Number.MAX_SAFE_INTEGER;
    const number = Number(monitor);
    return Number.isInteger(number) && number >= 0
        ? number
        : Number.MAX_SAFE_INTEGER;
}

/** Return a numeric workspace sort key, placing unknown workspaces last. */
function workspaceSortValue(workspace) {
    const number = Number(workspace) || 0;
    return number > 0 ? number : Number.MAX_SAFE_INTEGER;
}

/**
 * Read the corner badge IDs published by ACC's outline helper.
 * Parameters: file - outline_window_index.json beside the task payload.
 * Returns: a map from note identity to its 1-based workspace badge and desktop.
 */
function readWindowDisplayIds(file) {
    const ids = new Map();
    try {
        const [ok, bytes] = file.load_contents(null);
        const parsed = ok ? JSON.parse(new TextDecoder().decode(bytes)) : null;
        for (const row of Array.isArray(parsed?.windows) ? parsed.windows : []) {
            const identity = row?.note_id;
            const id = row?.number;
            if (identity && (typeof id === 'number' || typeof id === 'string') &&
                Number.isInteger(Number(id)) && Number(id) > 0)
                ids.set(String(identity), {id: Number(id), desktop: row.desktop});
        }
    } catch (_error) {
        // Missing or corrupt numbering must not hide open task rows.
    }
    return ids;
}

/**
 * Normalize the published display ID, keeping unknown IDs last.
 * Parameters: agent - one task row, optionally joined to its corner badge.
 * Returns: the numeric display ID, or a final-position sentinel.
 */
function windowDisplaySortValue(agent) {
    const id = agent.window_display_id;
    const result = id !== null && id !== undefined &&
        Number.isInteger(Number(id)) && Number(id) >= 0
        ? Number(id) : Number.MAX_SAFE_INTEGER;
    return result;
}

/**
 * Join the outline helper's IDs to scoped task rows without changing identity.
 * Parameters: payload - task rows scoped to live windows; ids - published ID map.
 * Returns: a copied payload with corner display IDs on matching desktops.
 */
function withWindowDisplayIds(payload, ids) {
    const providers = payload.providers.map(provider => ({...provider,
        agents: (provider.agents || []).map(agent => {
            const indexed = [agent.note_id, agent.tmux_name, agent.session_id, agent.key]
                .map(identity => ids.get(String(identity || ''))).find(Boolean);
            const matches = indexed && Number.isInteger(indexed.desktop) &&
                indexed.desktop + 1 === Number(agent.workspace);
            return {...agent, window_display_id: matches ? indexed.id : null};
        }),
    }));
    const result = {...payload, providers};
    return result;
}

/**
 * Sort workspace rows by numeric corner IDs before monitor fallbacks.
 * Parameters: left - first task/provider pair; right - second task/provider pair.
 * Returns: negative, zero, or positive ordering comparison.
 */
function compareWorkspaceEntries(left, right) {
    const result = workspaceSortValue(left.agent.workspace) - workspaceSortValue(right.agent.workspace) ||
        windowDisplaySortValue(left.agent) - windowDisplaySortValue(right.agent) ||
        monitorSortValue(left.agent.monitor) - monitorSortValue(right.agent.monitor) ||
        String(left.provider.label || '').localeCompare(String(right.provider.label || '')) ||
        String(left.agent.title || '').localeCompare(String(right.agent.title || ''));
    return result;
}

/**
 * Group once per workspace with numeric ID ordering inside each section.
 * Parameters: entries - task/provider pairs from all workspaces.
 * Returns: workspace groups sorted by workspace and numeric corner ID.
 */
function groupWorkspaceEntries(entries) {
    const groups = new Map();
    for (const entry of entries) {
        const workspace = Number(entry.agent.workspace) || 0;
        let group = groups.get(workspace);
        if (!group) {
            group = {workspace, entries: []};
            groups.set(workspace, group);
        }
        group.entries.push(entry);
    }

    const orderedGroups = Array.from(groups.values());
    for (const group of orderedGroups)
        group.entries.sort(compareWorkspaceEntries);
    orderedGroups.sort((left, right) => workspaceSortValue(left.workspace) -
        workspaceSortValue(right.workspace));
    return orderedGroups;
}

/**
 * Paint a dashed grey line across the separator's allocated content width.
 * Parameters: area - DrawingArea being repainted at the current menu width.
 * Returns: undefined after releasing its Cairo context.
 */
function paintMonitorSeparator(area) {
    const [width] = area.get_surface_size();
    const context = area.get_context();
    context.setSourceRGBA(0.63, 0.63, 0.63, 0.55);
    context.setLineWidth(1);
    context.setDash([6, 4], 0);
    context.moveTo(0, 0.5);
    context.lineTo(width, 0.5);
    context.stroke();
    context.$dispose();
}

/**
 * Build a compact dashed separator with the standard line's side margins.
 * Parameters: none.
 * Returns: a noninteractive native separator with a full-width dashed drawing.
 */
function buildMonitorSeparator() {
    const separator = new PopupMenu.PopupSeparatorMenuItem();
    separator.actor.add_style_class_name('ams-monitor-separator');
    for (const child of separator.get_children()) {
        if (child.has_style_class_name('popup-separator-menu-item-separator'))
            child.destroy();
    }
    const area = new St.DrawingArea({
        style_class: 'popup-separator-menu-item-separator ams-monitor-dashes',
        x_expand: true,
        y_align: Clutter.ActorAlign.CENTER,
    });
    area.connect('repaint', paintMonitorSeparator);
    separator.add_child(area);
    return separator;
}

/**
 * Match the ACC payload to every open agent window, preserving real workspace.
 *
 * Matching is one-to-one: a single terminal window can select at most one
 * payload row. This matters when historical sessions share the same summary;
 * one open window must never resurrect every matching detached session.
 * Provider and footer counts are recomputed from the selected rows.
 *
 * @param {object} payload - normalized document from {@link readPayload}.
 * @param {object[]} windows - live Shell window descriptors.
 * @returns {object} payload containing only agents backed by open windows.
 */
function scopePayloadToWindows(payload, windows) {
    const candidates = [];
    for (const provider of payload.providers) {
        const agents = Array.isArray(provider.agents) ? provider.agents : [];
        for (const agent of agents)
            candidates.push({provider, agent});
    }

    const selected = new Map();
    for (const window of windows) {
        let best = null;
        let bestScore = 0;
        for (const candidate of candidates) {
            if (selected.has(candidate.agent))
                continue;
            const score = agentWindowMatchScore(candidate.agent, window.title);
            if (score > bestScore) {
                best = candidate;
                bestScore = score;
            }
        }
        if (best)
            selected.set(best.agent, window);
    }

    const providers = [];
    let totalLive = 0;
    let totalRunning = 0;
    for (const provider of payload.providers) {
        const sourceAgents = Array.isArray(provider.agents) ? provider.agents : [];
        const agents = sourceAgents
            .filter(agent => selected.has(agent))
            .map(agent => {
                const match = selected.get(agent);
                return {
                    ...agent,
                    monitor: match.monitor,
                    workspace: match.workspace,
                    current_workspace: match.current,
                    focused: match.focused,
                };
            })
            .sort((left, right) => compareWorkspaceEntries(
                {provider, agent: left}, {provider, agent: right}));
        if (agents.length === 0)
            continue;
        const running = agents.filter(agent =>
            RUNNING_PHASES.has(String(agent.phase || '').trim().toUpperCase())).length;
        const attention = agents.filter(agent =>
            ATTENTION_PHASES.has(String(agent.phase || '').trim().toUpperCase())).length;
        providers.push({...provider, agents, live: agents.length, running, attention});
        totalLive += agents.length;
        totalRunning += running;
    }
    return {...payload, providers, totalLive, totalRunning};
}

/**
 * Count completed turns whose terminal has not received focus since finishing.
 *
 * Confirmed ready identities survive phase redraws and work-history updates.
 * Missing state baselines old completions as read; only a finish within 30
 * seconds can arm on first observation. Saved unread decisions remain valid
 * without an age limit. Legacy payloads retain their phase-based contract.
 * Keep the state by session identity so sorting and workspace moves cannot
 * transfer an unread mark to another terminal.
 *
 * @param {object} payload - agent rows scoped to open windows.
 * @param {Map<string, object>} states - prior phase and unread state by session.
 * @param {number} nowMillis - current time used to reject historical baselines.
 * @returns {number} completed turns still waiting to be opened.
 */
function unreadCompletionCount(payload, states, nowMillis = Date.now()) {
    let count = 0;
    for (const provider of payload.providers || []) {
        for (const agent of provider.agents || []) {
            const key = String(agent.session_id || agent.tmux_name || agent.key || '');
            if (!key) {
                agent.unread_completion = false;
                continue;
            }
            const phase = String(agent.phase || '').trim().toUpperCase();
            const previous = states.get(key);
            if (Object.prototype.hasOwnProperty.call(agent, 'ready_completed_at')) {
                const rawCompletion = String(agent.ready_completed_at || '');
                const completionMillis = Date.parse(rawCompletion);
                const confirmed = previous?.confirmed === true;
                const previousMillis = Date.parse(previous?.completedAt || '');
                const valid = Number.isFinite(completionMillis) &&
                    completionMillis <= nowMillis + 1000 &&
                    (!confirmed || !Number.isFinite(previousMillis) ||
                        completionMillis >= previousMillis);
                const completedAt = valid ? rawCompletion : '';
                const sameCompletion = completedAt &&
                    previous?.completedAt === completedAt;
                let unread = confirmed ? Boolean(previous.unread) : false;
                if (completedAt) {
                    if (sameCompletion && (confirmed || previous.unread === false)) {
                        unread = Boolean(previous.unread);
                    } else if (confirmed && previous.completedAt) {
                        // Only a later confirmed finish creates new attention.
                        unread = completionMillis > Date.parse(previous.completedAt);
                    } else {
                        // Migration and lost state must not rearm old DONE rows.
                        unread = nowMillis - completionMillis <= 30000;
                    }
                }
                if (agent.focused)
                    unread = false;
                agent.unread_completion = phase === 'DONE' && valid && unread;
                // Preserve identity through temporary RUNNING/empty-marker ticks.
                states.set(key, {phase, unread, confirmed: true,
                    completedAt: completedAt || (confirmed ? previous.completedAt : '')});
                if (agent.unread_completion)
                    count += 1;
                continue;
            }
            const completedAt = String(agent.last_worked_at || '');
            const sameCompletion = previous?.phase === 'DONE' &&
                (!completedAt || !previous.completedAt ||
                    previous.completedAt === completedAt);
            const unread = phase === 'DONE' && !agent.focused &&
                (sameCompletion ? previous.unread : true);
            agent.unread_completion = unread;
            states.set(key, {phase, unread, completedAt});
            if (unread)
                count += 1;
        }
    }
    return count;
}

/**
 * Count finished, unopened agents by workspace.
 *
 * Drives steady green `W<n>:<count>` badges in the panel and matching menu
 * headings. A finished turn disappears from both as soon as its terminal gets
 * focus; waiting, stopped, and already opened turns do not raise a marker.
 *
 * @param {object} payload - payload already scoped to live windows, so every
 *     agent carries the `workspace`/`current_workspace` of its own window.
 * @returns {Array<{workspace: number, count: number, current: boolean,
 *     label: string}>} one entry per workspace with at least one unread
 *     agent, in ascending workspace order. Agents whose window could not be
 *     resolved (workspace 0) are skipped — there is nowhere to switch to.
 */
function workspaceAttentionCounts(payload) {
    const providers = Array.isArray(payload?.providers) ? payload.providers : [];
    const byWorkspace = new Map();
    for (const provider of providers) {
        const agents = Array.isArray(provider.agents) ? provider.agents : [];
        for (const agent of agents) {
            if (!agent.unread_completion ||
                String(agent.phase || '').trim().toUpperCase() !== 'DONE')
                continue;
            const workspace = Number(agent.workspace) || 0;
            if (workspace <= 0)
                continue;
            const entry = byWorkspace.get(workspace) ||
                {workspace, count: 0, current: false, label: ''};
            entry.count += 1;
            entry.current = entry.current || Boolean(agent.current_workspace);
            byWorkspace.set(workspace, entry);
        }
    }
    const entries = [...byWorkspace.values()]
        .sort((left, right) => left.workspace - right.workspace);
    for (const entry of entries)
        entry.label = `W${entry.workspace}:${entry.count}`;
    return entries;
}

/** Return measured task progress independently of conversational completion.
 * Parameters: agent - scoped payload row.
 * Returns: a bounded measured fraction, or null when unavailable.
 */
function taskProgressValue(agent) {
    print(`agent=${agent}`);
    const ready = agent.workflow?.task_complete !== false && (agent.unread_completion ||
        (String(agent.phase || '').toUpperCase() === 'DONE' && agent.ready_completed_at));
    const value = agent.workflow?.fraction;
    const result = !ready && typeof value === 'number' && Number.isFinite(value)
        ? Math.max(0, Math.min(1, value)) : null;
    print(result);
    return result;
}

/**
 * Resolve measured progress for a compact panel bar, retaining confirmed finishes.
 * Parameters: agent - scoped open-window agent with published workflow data.
 * Returns: a fraction from zero to one, or null when progress is unavailable.
 */
function panelTaskProgressValue(agent) {
    print(`agent=${agent}`);
    const result = agent.workflow?.task_complete !== false && (agent.unread_completion ||
        (String(agent.phase || '').toUpperCase() === 'DONE' && agent.ready_completed_at))
        ? 1 : agent.workflow?.available === false ? null : taskProgressValue(agent);
    print(result);
    return result;
}

/**
 * Paint measured progress colors, lightening unread ready fills toward white.
 * Parameters: fraction - bounded progress or null; ready - unopened completion;
 * now - epoch milliseconds for the three-second pulse; agent - published state.
 * Returns: red, green and blue byte values for the progress color.
 */
function panelProgressColor(fraction, ready = false, now = Date.now(), agent = null) {
    print(`fraction=${fraction} ready=${ready} now=${now} agent=${agent}`);
    const interaction = agent?.workflow?.interaction?.mode === 'question';
    const red = [239, 68, 68];
    const green = [34, 197, 94];
    const interrupted = String(agent?.phase || '').trim().toUpperCase() === 'INTERRUPTED';
    const stateColor = interaction || interrupted || (!ready && fraction !== null) ? panelProgressStateColor(agent) : null;
    const base = stateColor || (fraction === null ? [48, 52, 64] :
        red.map((channel, index) => Math.round(channel + (green[index] - channel) * fraction)));
    const wave = ready && !interrupted && !interaction ? panelProgressWave(now) : 0;
    const result = base.map(channel => Math.round(channel + (255 - channel) * wave));
    print(result);
    return result;
}

/**
 * Match provider waits, drafts and interrupted fills to the shared badge palette.
 * Parameters: agent - published agent state, or null for ordinary progress.
 * Returns: state RGB bytes, or null to retain the measured progress gradient.
 */
function panelProgressStateColor(agent) {
    print(`agent=${agent}`);
    const phase = displayedPhase(agent);
    const hex = agent?.workflow?.interaction?.mode === 'question'
        ? PHASE_COLORS[agent.workflow.interaction.state === 'complete' ? 'ANSWERED' : 'QUESTION']
        : ['RATE', 'DRAFT', 'INTERRUPTED'].includes(phase) ? PHASE_COLORS[phase] : null;
    const result = hex ? [1, 3, 5].map(offset => parseInt(hex.slice(offset, offset + 2), 16)) : null;
    print(result);
    return result;
}

/**
 * Sample the shared zero-to-one-to-zero pulse over three seconds.
 * Parameters: now - epoch milliseconds.
 * Returns: the brightness wave from zero to one.
 */
function panelProgressWave(now) {
    const result = (1 - Math.cos(2 * Math.PI * (now % 3000) / 3000)) / 2;
    return result;
}

/**
 * Classify an agent whose current turn is actively doing work.
 * Parameters: agent - published open-window agent state.
 * Returns: true only for an active running turn.
 */
function panelProgressUnderway(agent) {
    const result = agent.run_active === true && String(agent.phase || '').toUpperCase() === 'RUNNING';
    return result;
}

/**
 * Identify a goal being pursued by an active running turn.
 * Parameters: agent - published agent state, or null.
 * Returns: true when the bar needs a purple goal edge.
 */
function panelProgressGoalActive(agent) {
    const result = Boolean(agent?.goal_active && panelProgressUnderway(agent));
    return result;
}

/**
 * Dim running bars while keeping ready bars fully opaque for color pulses.
 * Parameters: underway - whether the turn is active; now - epoch milliseconds;
 * stale - whether the snapshot is out of date; ready - unopened completion.
 * Returns: compositor opacity from 0 to 255, steadily dim when stale.
 */
function panelProgressOpacity(underway, now, stale, ready = false) {
    const alpha = underway && !ready ? 1 - 0.7 * panelProgressWave(now) : 1;
    const result = stale ? 128 : Math.round(255 * alpha);
    return result;
}

/**
 * Keep goal hosts steady so only their progress pixels animate.
 * Parameters: agent - published agent; now - animation time; stale - old data;
 * ready - unread completion flag.
 * Returns: compositor opacity byte, preserving ordinary and stale behavior.
 */
function panelProgressHostOpacity(agent, now, stale, ready) {
    const result = panelProgressGoalActive(agent) ? (stale ? 128 : 255) :
        panelProgressOpacity(panelProgressUnderway(agent), now, stale, ready);
    return result;
}

/**
 * Paint measured progress with its existing outer teal focus outline.
 * Parameters: area - taskbar DrawingArea carrying its current agent.
 * Returns: undefined after releasing the Cairo drawing context.
 */
function paintPanelTaskProgress(area) {
    print(`area=${area}`);
    const [width, height] = area.get_surface_size();
    const context = area.get_context();
    const fillWidth = Math.max(0, width - 2);
    const fillHeight = Math.max(0, height - 3);
    const fraction = panelTaskProgressValue(area._amsAgent);
    const ready = area._amsAgent.unread_completion === true && !area._amsStale;
    const fillAlpha = panelProgressGoalActive(area._amsAgent) && !area._amsStale ?
        panelProgressOpacity(true, area._amsPulseAt ?? Date.now(), false, ready) / 255 : 1;
    const color = panelProgressColor(fraction, ready, area._amsPulseAt ?? Date.now(), area._amsAgent)
        .map(channel => channel / 255);
    context.setSourceRGBA(...color, (fraction === null ? 0.45 : 0.22) * fillAlpha);
    context.rectangle(1, 1, fillWidth, fillHeight);
    context.fill();
    if (fraction !== null && fraction > 0) {
        context.setSourceRGBA(...color, fraction === 1 ? 1 : 0.72 * fillAlpha);
        context.rectangle(1, 1, fillWidth * fraction, fillHeight);
        context.fill();
    }
    if (area._amsAgent.focused === true) {
        // Reserve one pixel outside the fill for the CLI selection color.
        context.setSourceRGBA(0, 1, 230 / 255, 0.8);
        context.setLineWidth(1);
        context.rectangle(0.5, 0.5, Math.max(0, width - 1), Math.max(0, height - 2));
        context.stroke();
    }
    context.$dispose();
    print(undefined);
}

/**
 * Build a 50x8 progress fill with two pixels of bottom drawing room.
 * Parameters: agent - scoped open-window agent.
 * Returns: a DrawingArea showing actual progress on its next repaint.
 */
function buildPanelTaskProgress(agent) {
    const area = new St.DrawingArea({
        style_class: 'ams-panel-progress-bar',
        width: 52, height: 11,
        reactive: false,
        y_align: Clutter.ActorAlign.CENTER,
    });
    area._amsAgent = agent;
    area.connect('repaint', paintPanelTaskProgress);
    return area;
}

/**
 * Read seconds remaining until a rate-limited instance can launch.
 * Parameters: agent - published row with optional queued_reset_at; now - reference epoch milliseconds.
 * Returns: remaining seconds, or null when no future reset is published.
 */
function panelResetRemainingSeconds(agent, now) {
    const resetAt = Date.parse(agent && agent.queued_reset_at || '');
    const result = !Number.isNaN(resetAt) && resetAt > now ? (resetAt - now) / 1000 : null;
    return result;
}

/**
 * Report work that has already finished, including a completed workflow plan.
 * Parameters: agent - published row with phase, completion, and optional workflow counts.
 * Returns: true when the task is done and must not be presented as waiting to start.
 */
function taskMarkedDone(agent) {
    print(`agent=${agent}`);
    const phase = String(agent?.phase || '').trim().toUpperCase();
    const completed = agent?.workflow?.phase_completed;
    const total = agent?.workflow?.phase_total;
    const workflowDone = Number.isInteger(completed) && Number.isInteger(total) &&
        total > 0 && completed >= total;
    const result = agent?.workflow?.task_complete !== false && (phase === 'DONE' || Boolean(agent?.ready_completed_at) || workflowDone);
    print(result);
    return result;
}

/**
 * Read the launch wait only for an instance that has not started and has not finished.
 * Parameters: agent - published row; now - reference epoch milliseconds.
 * Returns: seconds until launch, or null when this row is running, done, or has no reset.
 */
function panelLaunchWaitSeconds(agent, now) {
    const remaining = panelResetRemainingSeconds(agent, now);
    const result = remaining !== null && !taskMarkedDone(agent) && !panelProgressUnderway(agent)
        ? remaining : null;
    return result;
}

/**
 * Format a common cell forecast, including retained Harbor terminal status.
 * A future launch countdown replaces this lower label, so time left stays off the bottom.
 * Parameters: agent - common panel task with an ACC estimate or independent Harbor forecast.
 * Returns: compact agent duration or Harbor forecast/status text, or empty text.
 */
const panelAgentEstimateText = (agent) => {
    if (agent.harbor)
        return formatHarborEstimate(agent.harbor);
    const label = panelLaunchWaitSeconds(agent, Date.now()) === null &&
        panelProgressUnderway(agent) && agent.task_time_estimate?.status === 'estimated'
        ? taskEstimateText(agent) : '';
    const duration = label.match(/^[~<>≤≥]?\s*(?:\d+h(?:\s*\d+m)?|\d+m)/)?.[0] || '';
    const text = duration.replace(/\s+/g, '');
    return text;
};

/**
 * Format a known nonnegative runtime in hours and whole minutes.
 * Parameters: seconds - elapsed or recorded active seconds, or an unavailable value.
 * Returns: HH:MM text without seconds, or empty text when the duration is unknown.
 */
function formatPanelRunDuration(seconds) {
    let text = '';
    if (typeof seconds === 'number' && Number.isFinite(seconds) && seconds >= 0) {
        const totalMinutes = Math.floor(seconds / 60);
        const hours = Math.floor(totalMinutes / 60);
        text = `${String(hours).padStart(2, '0')}:${String(totalMinutes % 60).padStart(2, '0')}`;
    }
    return text;
}

/**
 * Format the upper label for a Harbor job or ACC task.
 * Parameters: agent - panel task with run and finish timestamps or Harbor counts;
 * now - reference epoch milliseconds, defaulting to the current clock.
 * Returns: Harbor counts, runtime, launch countdown, completion age, or empty text.
 */
const panelAgentRuntimeText = (agent, now = Date.now()) => {
    if (agent.harbor)
        return harborCountText(agent.harbor);
    const remaining = panelLaunchWaitSeconds(agent, now);
    if (remaining !== null)
        return `-${formatPanelRunDuration(Math.max(remaining, 60))}`;
    if (panelProgressUnderway(agent)) {
        const started = Date.parse(agent.run_started_at || '');
        const seconds = Number.isFinite(started)
            ? Math.max(0, (now - started) / 1000)
            : agent.task_time_estimate?.observed_active_seconds;
        return formatPanelRunDuration(seconds);
    }
    return panelCompletionAgeText(Date.parse(agent.ready_completed_at || ''), now,
        String(agent.phase || '').trim().toUpperCase() === 'DONE' && agent.workflow?.task_complete !== false);
};

/**
 * Match the CLI's compact completion age for a confirmed Ready task.
 * Parameters: completedAt - parsed completion epoch milliseconds; now - reference epoch milliseconds;
 * ready - whether this turn finished the task.
 * Returns: compact completion age or empty text.
 */
const panelCompletionAgeText = (completedAt, now, ready) => {
    if (!ready || !Number.isFinite(completedAt) || completedAt > now)
        return '';
    const seconds = Math.floor((now - completedAt) / 1000);
    if (seconds < 60)
        return `${seconds}s ago`;
    if (seconds < 3600)
        return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400)
        return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
};

/**
 * Color the upper label, retaining pink Harbor counts and the normal agent completion wave.
 * Parameters: agent - scoped task state; now - animation epoch milliseconds;
 * stale - whether the published task state is too old to animate.
 * Returns: pink Harbor count, completion RGB, goal purple, queued-reset green, interruption red, or running yellow.
 */
const panelAgentRuntimeColor = (agent, now = Date.now(), stale = false) => {
    if (agent?.workflow?.interaction?.mode === 'question')
        return PHASE_COLORS[agent.workflow.interaction.state === 'complete' ? 'ANSWERED' : 'QUESTION'];
    if (agent.harbor)
        return '#f472b6';
    let color = panelProgressGoalActive(agent) ? PHASE_COLORS.GOAL : PHASE_COLORS.RUNNING;
    if (panelLaunchWaitSeconds(agent, now) !== null) {
        const phase = String(agent?.phase || '').trim().toUpperCase();
        color = phase === 'INTERRUPTED' ? PHASE_COLORS.INTERRUPTED : PHASE_COLORS.QUEUED_RESET;
    } else if (String(agent.phase || '').toUpperCase() === 'DONE' && agent.ready_completed_at) {
        const rgb = panelProgressColor(1, agent.unread_completion === true && !stale, now, agent);
        color = `rgb(${rgb.join(', ')})`;
    }
    return color;
};

/**
 * Resolve the published instance numbers that distinguish open bars.
 * Parameters: entries - scoped agent and provider pairs; codexInstanceCount - registered Codex accounts.
 * Returns: a map from each numbered bar's key to its display number.
 */
function panelInstanceNumbers(entries, codexInstanceCount) {
    print(`entries=${entries} codexInstanceCount=${codexInstanceCount}`);
    const projectGroups = new Map();
    const result = new Map();
    for (const {agent, provider, key} of entries) {
        const kind = agent.kind || provider.kind;
        const showAccount = kind === 'codex' && codexInstanceCount > 1;
        const number = Number(showAccount ? agent.codex_instance : agent.instance);
        const displayNumber = Number.isSafeInteger(number) && number > 0 ? number : null;
        if (showAccount) {
            if (displayNumber !== null)
                result.set(key, displayNumber);
        } else if (!agent.harbor) {
            const groupKey = JSON.stringify([kind, agent.project || '']);
            if (!projectGroups.has(groupKey))
                projectGroups.set(groupKey, []);
            projectGroups.get(groupKey).push({key, displayNumber});
        }
    }
    for (const group of projectGroups.values()) {
        if (group.length > 1) {
            for (const {key, displayNumber} of group) {
                if (displayNumber !== null)
                    result.set(key, displayNumber);
            }
        }
    }
    print(result);
    return result;
}

/**
 * Read the local usage poller's snapshot for the account badges.
 * Parameters: file - agentic-cli-status status.json.
 * Returns: the parsed snapshot, or null when unavailable.
 */
function readPanelQuotaSnapshot(file) {
    print(`file=${file}`);
    let snapshot = null;
    try {
        const [ok, bytes] = file.load_contents(null);
        if (ok)
            snapshot = JSON.parse(new TextDecoder().decode(bytes));
    } catch (_error) {
        // The optional usage poller may be absent or replacing its snapshot.
    }
    print(snapshot);
    return snapshot;
}

/**
 * Classify exhausted Codex logins from their own allowance and reset time.
 * Parameters: snapshot - parsed usage poller document; now - epoch seconds.
 * Returns: the set of exhausted account numbers.
 */
function exhaustedCodexAccounts(snapshot, now) {
    print(`snapshot=${snapshot} now=${now}`);
    const result = new Set();
    const codex = snapshot?.clis?.codex;
    if (codex?.enabled !== false && Array.isArray(codex?.instances)) {
        for (const account of codex.instances) {
            const id = account?.id;
            if (!Number.isSafeInteger(id) || id < 1 || account.error)
                continue;
            const windows = Array.isArray(account.windows) ? account.windows : [];
            if (account.credits?.blocked === true || windows.some(window =>
                ['5h', 'week', 'month'].includes(window?.label) &&
                Number.isFinite(window.used_percent) && window.used_percent >= 100 &&
                Number.isFinite(window.resets_at) && window.resets_at > now))
                result.add(id);
        }
    }
    print([...result]);
    return result;
}

/**
 * Stack runtime, branded progress and the instance and forecast footer in the panel height.
 * Parameters: agent - scoped open-window agent whose bar and timing are displayed;
 * focusAgent - callback selecting the exact agent note, or null for noninteractive rendering;
 * toggleHidden - callback changing panel visibility, or null for an inert cell.
 * Returns: a fixed-width click and hover target with retained progress and labels.
 */
function buildPanelTaskCell(agent, focusAgent = null, toggleHidden = null) {
    print(`agent=${agent} focusAgent=${focusAgent} toggleHidden=${toggleHidden}`);
    const cell = new St.Widget({
        style_class: 'ams-panel-task-cell',
        layout_manager: new Clutter.BinLayout(),
        width: 52,
        y_expand: true,
        y_align: Clutter.ActorAlign.FILL,
        clip_to_allocation: true,
        reactive: Boolean(toggleHidden || (focusAgent && String(agent.note_id || '').trim())),
        can_focus: Boolean(focusAgent && String(agent.note_id || '').trim()),
        track_hover: Boolean(focusAgent && String(agent.note_id || '').trim()),
    });
    const logo = new St.Icon({
        style_class: 'ams-panel-task-logo',
        icon_size: cell.width,
        x_expand: true,
        y_expand: true,
        x_align: Clutter.ActorAlign.CENTER,
        y_align: Clutter.ActorAlign.CENTER,
        reactive: false,
        visible: false,
    });
    const bar = buildPanelTaskProgress(agent);
    const footer = new St.BoxLayout({
        style_class: 'ams-panel-task-footer',
        x_expand: true,
        y_expand: true,
        x_align: Clutter.ActorAlign.FILL,
        y_align: Clutter.ActorAlign.END,
        reactive: false,
    });
    const instance = new St.Label({
        style_class: 'ams-panel-agent-instance',
        text: '',
        visible: false,
        x_align: Clutter.ActorAlign.START,
        y_align: Clutter.ActorAlign.END,
        reactive: false,
    });
    const estimate = new St.Label({
        style_class: 'ams-panel-agent-estimate',
        text: '',
        visible: false,
        x_expand: true,
        x_align: Clutter.ActorAlign.END,
        y_align: Clutter.ActorAlign.END,
        reactive: false,
    });
    estimate.clutter_text.ellipsize = Pango.EllipsizeMode.END;
    const runtime = new St.Label({
        style_class: 'ams-panel-agent-runtime',
        text: '',
        visible: false,
        x_expand: true,
        y_expand: true,
        x_align: Clutter.ActorAlign.CENTER,
        y_align: Clutter.ActorAlign.START,
        reactive: false,
    });
    cell.connect('button-press-event', (_cell, event) => {
        if (event.get_button() === 3 && cell._amsConfirmation) {
            cell._amsConfirmation();
            return Clutter.EVENT_STOP;
        }
        if (event.get_button() === 1 && (event.get_state?.() & Clutter.ModifierType.CONTROL_MASK) && toggleHidden) {
            toggleHidden();
            return Clutter.EVENT_STOP;
        }
        if (event.get_button() !== 1)
            return Clutter.EVENT_PROPAGATE;
        const noteId = String(bar._amsAgent.note_id || '').trim();
        if (noteId && focusAgent)
            focusAgent(noteId);
        return Clutter.EVENT_STOP;
    });
    cell._amsBar = bar;
    bar._amsLogo = logo;
    bar._amsInstanceLabel = instance;
    bar._amsEstimateLabel = estimate;
    bar._amsRuntimeLabel = runtime;
    // Keep the watermark below the fill and both rows of labels.
    cell.add_child(logo);
    cell.add_child(bar);
    footer.add_child(instance);
    footer.add_child(estimate);
    cell.add_child(footer);
    cell.add_child(runtime);
    print(cell);
    return cell;
}

/**
 * Sample the blue-to-violet workspace-label gradient from W1 through W8.
 * Parameters: workspace - normalized numeric workspace identity, or zero if unknown.
 * Returns: a CSS hex color, neutral for unknown and capped at W8 for higher workspaces.
 */
function panelWorkspaceColor(workspace) {
    if (!Number.isInteger(workspace) || workspace < 1) {
        const result = '#9a9996';
        return result;
    }
    const fraction = Math.min(1, (workspace - 1) / 7);
    const start = [96, 165, 250];
    const end = [192, 132, 252];
    const channels = start.map((value, index) => Math.round(value + (end[index] - value) * fraction));
    const result = '#' + channels.map(value => value.toString(16).padStart(2, '0')).join('');
    return result;
}

/**
 * Draw larger round grey workspace dots through the full allocated panel height.
 * Parameters: area - divider DrawingArea with its current allocated size.
 * Returns: undefined after releasing the Cairo drawing context.
 */
function paintPanelWorkspaceSeparator(area) {
    const [width, height] = area.get_surface_size();
    const context = area.get_context();
    context.setSourceRGBA(0.63, 0.63, 0.63, 0.55);
    context.setLineWidth(2);
    context.setLineCap(Cairo.LineCap.ROUND);
    // GJS requires positive dash lengths; round caps turn this tiny dash into a dot.
    context.setDash([0.01, 5.99], 0);
    context.moveTo(width / 2, 0);
    context.lineTo(width / 2, height);
    context.stroke();
    context.$dispose();
}

/**
 * Build an inert full-height dotted separator between workspace progress groups.
 * Parameters: none.
 * Returns: a full-height DrawingArea with no label or added interaction.
 */
function buildPanelWorkspaceSeparator() {
    const area = new St.DrawingArea({
        style_class: 'ams-panel-workspace-divider',
        width: 6,
        reactive: false,
        y_expand: true,
        y_align: Clutter.ActorAlign.FILL,
    });
    area.connect('repaint', paintPanelWorkspaceSeparator);
    return area;
}

/**
 * Paint a solid one-pixel teal line through the centered half of the monitor boundary.
 * Parameters: area - monitor divider with its current allocated panel size.
 * Returns: undefined after releasing the Cairo context.
 */
function paintPanelMonitorSeparator(area) {
    const [width, height] = area.get_surface_size();
    const context = area.get_context();
    context.setSourceRGBA(0, 1, 230 / 255, 0.8);
    context.setLineWidth(1);
    context.moveTo(width / 2, height / 4);
    context.lineTo(width / 2, height * 3 / 4);
    context.stroke();
    context.$dispose();
}

/**
 * Build a centered half-height teal monitor boundary with twice the normal bar gap.
 * Parameters: none.
 * Returns: an inert DrawingArea stretching through the allocated panel height.
 */
function buildPanelMonitorSeparator() {
    const area = new St.DrawingArea({
        style_class: 'ams-panel-monitor-divider',
        // The two adjacent six-pixel gaps double the spacing around this line.
        width: 1,
        reactive: false,
        y_expand: true,
        y_align: Clutter.ActorAlign.FILL,
    });
    area.connect('repaint', paintPanelMonitorSeparator);
    return area;
}

/** Lay out measured phase/action blocks with the terminal overlay's palette. */
function taskProgressRectangles(agent, width) {
    const phases = ['Plan', 'Establish worktree', 'Write code', 'Write documentation'];
    const palette = ['92, 168, 255', '174, 129, 255', '55, 211, 190', '238, 171, 87'];
    const segments = agent.workflow?.segments;
    if (!Array.isArray(segments) || !segments.length || !Number.isFinite(width) || width <= 0 ||
        segments.some(s => !s || !phases.includes(s.phase) ||
            !['complete', 'in_progress', 'pending', 'skipped'].includes(s.status)))
        return [];
    width = Math.floor(width);
    let gaps = segments.slice(1).map((s, i) => s.phase === segments[i].phase ? 2 : 7);
    const count = segments.length;
    if (width < count * 3 + gaps.reduce((a, b) => a + b, 0))
        gaps = gaps.map(gap => Math.min(1, gap));
    if (width < count + gaps.reduce((a, b) => a + b, 0))
        gaps = gaps.map(() => 0);
    const usable = width - gaps.reduce((a, b) => a + b, 0);
    let inset = 0;
    const rects = [];
    segments.forEach((segment, i) => {
        const x = Math.floor(usable * i / count) + inset;
        const right = Math.floor(usable * (i + 1) / count) + inset;
        const color = segment.status === 'complete' ? palette[phases.indexOf(segment.phase)] :
            segment.status === 'in_progress' ? '255, 198, 79' :
                segment.status === 'skipped' ? '72, 77, 89' : '48, 52, 64';
        if (right > x)
            rects.push({x, width: right - x, color});
        inset += gaps[i] || 0;
    });
    return rects;
}

/** Read ACC's local forecast for timed tasks; idle/completed tasks have none. */
function taskEstimateText(agent) {
    const timed = (agent?.phase === 'RUNNING' && agent?.run_active) ||
        Date.parse(agent?.queued_reset_at || '') > Date.now();
    const estimate = agent?.task_time_estimate;
    if (!timed || !['estimated', 'learning', 'overrun'].includes(estimate?.status) ||
        typeof estimate?.label !== 'string')
        return '';
    return estimate.label.trim();
}

/** Format the active session's goal or latest prompt for the menu or hover.
 * Parameters: agent - published session row; compact - whether this is a hover preview.
 * Returns: display text and its CSS color, or null when no active context exists.
 */
function sessionContextPresentation(agent, compact) {
    print(`agent=${agent} compact=${compact}`);
    const context = agent?.session_context;
    let result = null;
    if (String(agent?.phase || '').trim().toUpperCase() === 'RUNNING' &&
        typeof context?.text === 'string') {
        let text = context.text.replace(/\s+/g, ' ').trim();
        const limit = compact ? Infinity : 110;
        if (text.length > limit) {
            const prefix = text.slice(0, limit - 1);
            const boundary = prefix.lastIndexOf(' ');
            text = (boundary > limit / 2 ? prefix.slice(0, boundary) : prefix).trimEnd() + '…';
        }
        if (text)
            result = {text, color: context.kind === 'goal' ? '#c59af4' : '#a9bac8'};
    }
    print(JSON.stringify(result));
    return result;
}

/** Update the background, completion glow and context without replacing row actors.
 * Parameters: widgets - retained row actors; agent - current session snapshot.
 * Returns: undefined.
 */
function updateTaskProgress(widgets, agent) {
    print(`widgets=${widgets} agent=${agent}`);
    widgets.agent = agent;
    const fraction = taskProgressValue(agent);
    widgets.fraction = fraction ?? 0;
    widgets.track.visible = fraction !== null;
    widgets.resize();
    if (agent.unread_completion)
        widgets.item.actor.add_style_class_name('ams-agent-unread');
    else
        widgets.item.actor.remove_style_class_name('ams-agent-unread');
    widgets.item.accessible_name = [agent.title, agent.workflow?.label].filter(Boolean).join(' · ');
    widgets.badge.text = agentPhaseBadgeText(agent);
    widgets.badge.set_style(`color: ${agentPhaseBadgeColor(agent)};`);
    widgets.badge._amsAgent = agent;
    if (widgets.estimateLabel) {
        widgets.estimateLabel.text = taskEstimateText(agent);
        widgets.estimateLabel.visible = Boolean(widgets.estimateLabel.text);
    }
    if (widgets.contextLabel) {
        const context = sessionContextPresentation(agent, widgets.compactContext);
        widgets.contextLabel.text = context?.text || '';
        widgets.contextLabel.visible = Boolean(context);
        if (context)
            widgets.contextLabel.set_style(`color: ${context.color};`);
    }
    print(undefined);
}

/** Layer a measured fill behind task text without changing its click target.
 * Parameters: item - menu row; row - text container; badge - status label; agent - task snapshot.
 * Returns: retained progress widgets whose geometry updates only while attached to the stage.
 */
function buildTaskProgressLayer(item, row, badge, agent) {
    const stack = new St.Widget({layout_manager: new Clutter.BinLayout(), x_expand: true});
    const track = new St.Widget({style_class: 'ams-task-progress',
        layout_manager: new Clutter.FixedLayout(), x_expand: true, y_expand: true,
        reactive: false});
    const fill = new St.Widget({style_class: 'ams-task-progress-fill', reactive: false});
    track.add_child(fill);
    stack.add_child(track);
    row.x_expand = true;
    stack.add_child(row);
    item.add_child(stack);
    const widgets = {item, track, fill, badge, blocks: [], fraction: 0,
    /** Size attached progress actors without requesting themes during construction or teardown.
     * Parameters: this - retained progress widget group.
     * Returns: undefined.
     */
    resize() {
        if (!track.get_stage())
            return;
        const width = track.get_width(), height = track.get_height();
        const rects = taskProgressRectangles(this.agent, width);
        fill.visible = track.visible && this.fraction > 0 && !rects.length;
        fill.set_position(0, 0);
        fill.set_size(Math.floor(width * this.fraction), height);
        while (this.blocks.length > rects.length)
            this.blocks.pop().destroy();
        rects.forEach((rect, i) => {
            if (!this.blocks[i]) {
                const block = new St.Widget({style_class: 'ams-task-progress-block', reactive: false});
                track.add_child(block);
                this.blocks.push(block);
            }
            const block = this.blocks[i];
            block.set_position(rect.x, 0);
            block.set_size(rect.width, height);
            block.set_style(`background-color: rgba(${rect.color}, 0.24);`);
        });
    }};
    track.connect('notify::allocation', () => widgets.resize());
    updateTaskProgress(widgets, agent);
    return widgets;
}

/**
 * Include regular Shell shortcuts in the ACC menu's modal popup grab.
 *
 * Parameters: manager - panel popup manager; isOpen - whether the menu is opening; originalActionMode - the manager's original mode.
 * Returns: undefined after updating the popup action mode.
 */
function setStatusMenuActionMode(manager, isOpen, originalActionMode) {
    manager._grabParams.actionMode = isOpen
        ? originalActionMode | Shell.ActionMode.NORMAL
        : originalActionMode;
    return undefined;
}

/** Build the native completion command from the selected published session identity.
 * Parameters: agent - confirmed panel task with its stable registered session id.
 * Returns: public session-only pa completion argv, or null when the session id is absent.
 */
function completionCommand(agent) {
    const sessionId = typeof agent?.session_id === 'string' ? agent.session_id.trim() : '';
    const result = sessionId ? ['pa', 'mark-in-progress-done', '--close-session', sessionId] : null;
    return result;
}

/** Extract the native completion rejection after its diagnostic lines.
 * Parameters: stderr - captured native command error output, or empty text.
 * Returns: its final nonblank message or a useful generic native-failure reason.
 */
function completionErrorMessage(stderr) {
    const result = String(stderr || '').trim().split('\n').at(-1) || 'Native task completion failed';
    return result;
}

const AccModelStatusButton = GObject.registerClass(
class AccModelStatusButton extends PanelMenu.Button {
    /**
     * Build the panel button and identify the loaded completion routing.
     *
     * Parameters: this - button instance; extension - owning extension for the icon directory.
     * Returns: undefined after initialization.
     */
    _init(extension) {
        super._init(0.5, 'ACC Model Status');
        print('acc-model-status: confirmed-close native session routing loaded');
        this.track_hover = false;
        // Native panel hover and open-menu styles must not paint every instance.
        this.style = 'background-color: transparent; box-shadow: none;';
        this._iconsDir = extension.dir.get_child('icons');
        this._file = payloadFile();
        this._unreadFile = this._file.get_parent().get_child(UNREAD_FILENAME);
        this._monitor = null;
        this._pollId = 0;
        this._refreshId = 0;
        this._runClockId = 0;
        this._panelPulseId = 0;
        this._unreadStates = readUnreadStates(this._unreadFile);
        this._unreadCount = 0;
        this._runClockActors = [];
        this._taskRows = new Map();
        this._shellSignals = [];
        this._windowSignals = new Map();

        this._box = new St.BoxLayout({style_class: 'ams-box panel-status-menu-box'});
        this.add_child(this._box);

        const menuManager = Main.panel.menuManager;
        this._statusMenuActionMode = menuManager._grabParams.actionMode;
        // Run before addToStatusArea registers the manager's open callback.
        this.menu.connect('open-state-changed', (_menu, isOpen) => {
            setStatusMenuActionMode(menuManager, isOpen, this._statusMenuActionMode);
            if (isOpen)
                this._hideTaskHover();
            else
                this._queueRefresh();
        });

        this._startWatching();
        this._startWindowWatching();
        this._refresh();
    }

    /**
     * Open the detailed agent menu only on a secondary pointer press.
     * Parameters: this - model-status button; event - incoming Clutter input event.
     * Returns: EVENT_STOP for panel pointer presses, otherwise EVENT_PROPAGATE.
     */
    vfunc_event(event) {
        const type = event.type();
        let result = Clutter.EVENT_PROPAGATE;
        if (type === Clutter.EventType.BUTTON_PRESS) {
            if (event.get_button() === 3)
                this.menu?.toggle();
            result = Clutter.EVENT_STOP;
        } else if (type === Clutter.EventType.TOUCH_BEGIN) {
            result = Clutter.EVENT_STOP;
        } else if (type === Clutter.EventType.LEAVE) {
            this._queueRefresh();
        }
        return result;
    }

    /**
     * Watch the payload file and start the safety-net poll.
     *
     * The Gio monitor gives near-instant updates; the timer covers monitors
     * that miss events on some filesystems and the not-yet-created case.
     */
    _startWatching() {
        try {
            this._monitor = this._file.monitor_file(Gio.FileMonitorFlags.NONE, null);
            this._monitor.connect('changed', () => this._queueRefresh());
        } catch (error) {
            logError(error, 'acc-model-status: file monitor unavailable, polling only');
            this._monitor = null;
        }
        this._pollId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, POLL_SECONDS, () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });
    }

    /** Connect one Shell signal and retain it for deterministic teardown. */
    _connectShell(object, signal, callback) {
        this._shellSignals.push([object, object.connect(signal, callback)]);
    }

    /** Watch active-workspace changes and the lifecycle of every Shell window. */
    _startWindowWatching() {
        this._connectShell(global.display, 'notify::focus-window', () =>
            this._queueRefresh());
        this._connectShell(global.workspace_manager, 'active-workspace-changed', () =>
            this._queueRefresh());
        this._connectShell(global.display, 'window-created', (_display, win) => {
            this._trackWindow(win);
            this._queueRefresh();
        });
        for (const actor of global.get_window_actors()) {
            if (actor.meta_window)
                this._trackWindow(actor.meta_window);
        }
    }

    /**
     * Track identity, workspace, and position changes until a window is
     * unmanaged.
     *
     * @param {Meta.Window} win - live Shell window.
     */
    _trackWindow(win) {
        if (!win || this._windowSignals.has(win))
            return;
        const ids = [
            win.connect('workspace-changed', () => this._queueRefresh()),
            win.connect('position-changed', () => this._queueRefresh()),
            win.connect('notify::title', () => this._queueRefresh()),
            win.connect('unmanaged', () => {
                const stored = this._windowSignals.get(win) || [];
                for (const id of stored)
                    win.disconnect(id);
                this._windowSignals.delete(win);
                this._queueRefresh();
            }),
        ];
        this._windowSignals.set(win, ids);
    }

    /**
     * Return every open window's title, monitor, actual workspace, and marker.
     *
     * GNOME monitor and workspace indexes are zero-based; workspace menu
     * headings are one-based. Sorting by monitor index puts monitor 1 first.
     * A sticky window is considered part of the current workspace because
     * that is where it is presently visible.
     */
    _openWindowDescriptors() {
        const activeWorkspace = global.workspace_manager.get_active_workspace();
        const activeWorkspaceNumber = activeWorkspace.index() + 1;
        const focusedWindow = global.display.focus_window;
        const windows = [];
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window;
            if (!win)
                continue;
            const title = win.get_title();
            if (!title)
                continue;
            const workspaceObject = win.get_workspace();
            let workspace = workspaceObject ? workspaceObject.index() + 1 : 0;
            const monitor = win.get_monitor();
            const current = win.located_on_workspace(activeWorkspace);
            if (current && typeof win.is_on_all_workspaces === 'function' &&
                win.is_on_all_workspaces())
                workspace = activeWorkspaceNumber;
            windows.push({title, monitor, workspace, current,
                focused: win === focusedWindow});
        }
        return windows;
    }

    /** Coalesce monitor event bursts (create/write/close) into one redraw. */
    _queueRefresh() {
        if (this._refreshId)
            return;
        this._refreshId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 120, () => {
            this._refreshId = 0;
            this._refresh();
            return GLib.SOURCE_REMOVE;
        });
    }

    /**
     * Resolve a provider icon shipped with the extension.
     *
     * @param {string} name - icon name from the payload.
     * @returns {Gio.Icon} file icon, falling back to the generic mark.
     */
    _icon(name) {
        const safe = typeof name === 'string' && /^acc-provider-[a-z0-9_-]+-symbolic$/.test(name)
            ? name
            : FALLBACK_ICON;
        let child = this._iconsDir.get_child(`${safe}.svg`);
        if (!child.query_exists(null))
            child = this._iconsDir.get_child(`${FALLBACK_ICON}.svg`);
        return new Gio.FileIcon({file: child});
    }

    /** Refresh task widgets while the menu stays open and clickable.
     * Parameters: this - status button; payload - current visible sessions.
     * Returns: undefined.
     */
    _updateTaskRows(payload) {
        print(`this=${this} payload=${payload}`);
        const agents = new Map();
        for (const provider of payload?.providers || []) {
            for (const agent of provider.agents || []) {
                const key = String(agent.session_id || agent.tmux_name || agent.key || '');
                agents.set(key, agent);
            }
        }
        for (const [key, widgets] of this._taskRows) {
            const agent = agents.get(key) || {...widgets.agent, workflow: null, unread_completion: false, session_context: null};
            updateTaskProgress(widgets, agent);
        }
        print(undefined);
    }

    /**
     * Update tasks and menus while preserving the hovered panel allocation.
     * Parameters: this - model-status button whose panel and menu are refreshed.
     * Returns: undefined.
     */
    _refresh() {
        const published = readPayload(this._file);
        const windows = this._openWindowDescriptors();
        const payload = published
            ? withWindowDisplayIds(scopePayloadToWindows(published, windows),
                readWindowDisplayIds(this._file.get_parent().get_child('outline_window_index.json')))
            : null;
        const stale = published ? ageSeconds(published.updatedAt) > STALE_SECONDS : false;
        const previousUnreadState = JSON.stringify([...this._unreadStates]);
        const previousUnreadCount = this._unreadCount;
        this._unreadCount = payload
            ? unreadCompletionCount(payload, this._unreadStates)
            : 0;
        if (previousUnreadState !== JSON.stringify([...this._unreadStates]))
            saveUnreadStates(this._unreadFile, this._unreadStates);
        if (previousUnreadCount !== this._unreadCount)
            log(`acc-model-status: unread completions ${previousUnreadCount} -> ${this._unreadCount}`);
        if (this.menu.isOpen) {
            this._syncMuteIndicator(Boolean(payload?.announcementsMuted));
            // Preserve row identities while refreshing progress and glow together.
            this._updateTaskRows(payload);
            this._updatePanelProgress(payload, stale);
            return;
        }
        const hovering = Boolean(payload?.providers.length) && [...(this._panelProgressBars?.values() || [])]
            .some(bar => bar.get_parent().hover);
        this._taskRows.clear();
        if (!hovering) {
            if (this._panelProgress) {
                // Detaching a hovered subtree clears its highlight, even if reattached.
                for (const child of this._box.get_children()) {
                    if (child !== this._panelProgress && child !== this._muteButton)
                        child.destroy();
                }
            } else {
                this._box.destroy_all_children();
                this._muteIcon = null;
            }
        }
        this._syncMuteIndicator(Boolean(payload?.announcementsMuted));
        this.menu.removeAll();
        // Run clocks are re-collected on every repaint; never keep stale ones.
        this._runClockActors = [];

        if (payload)
            this._renderHarnessRates(payload);

        if (!payload || payload.providers.length === 0) {
            const rateRows = panelHarnessRateRows(payload);
            if (rateRows.length === 0)
                this._renderEmpty(payload);
            else {
                this._renderPanelHarnessRates(payload, stale, new Set());
                this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
                    'No open agent terminals', {reactive: false}));
            }
            this._syncRunClockTimer();
            this._updatePanelProgress(payload, stale);
            return;
        }

        // Completion counts belong to workspace headings in the drop-down.
        const attention = workspaceAttentionCounts(payload);
        const shownKinds = new Set();
        for (const provider of payload.providers) {
            shownKinds.add(provider.kind);
            if (!hovering)
                this._renderProvider(provider, stale, payload);
        }
        if (!hovering)
            this._renderPanelHarnessRates(payload, stale, shownKinds);
        this._updatePanelProgress(payload, stale);
        this._renderMenu(payload, stale, attention);
        this._syncRunClockTimer();
    }

    /** Show the colored task row in a box anchored above its panel cell.
     * Parameters: this - status button; cell - hovered task actor; agent - current task;
     * provider - task provider; codexInstanceCount - account count for the instance color.
     * Returns: undefined.
     */
    _syncTaskHover(cell, agent, provider, codexInstanceCount) {
        if (this._taskConfirmation) {
            return;
        }
        if (!cell.hover || this.menu.isOpen) {
            if (this._taskHoverCell === cell)
                this._hideTaskHover();
            return;
        }
        this._hideTaskHover();
        const item = this._agentMenuItem(agent, provider, codexInstanceCount, false);
        item.add_style_class_name('ams-task-hover-content');
        item.reactive = false;
        item.can_focus = false;
        const popup = new BoxPointer.BoxPointer(St.Side.BOTTOM, {reactive: false});
        popup.add_style_class_name('popup-menu-boxpointer');
        popup.add_style_class_name('ams-task-hover');
        popup.bin.set_child(item);
        Main.layoutManager.addChrome(popup);
        popup.setPosition(cell, 0.5);
        this._taskHoverCell = cell;
        this._taskHoverPopup = popup;
        popup.open(BoxPointer.PopupAnimation.NONE);
    }

    /** Remove the anchored preview and release its actor.
     * Parameters: this - status button owning the optional preview.
     * Returns: undefined.
     */
    _hideTaskHover() {
        this._taskHoverPopup?.destroy();
        this._taskHoverPopup = null;
        this._taskHoverCell = null;
    }

    /** Arm an above-instance confirmation containing the task's colored details.
     * Parameters: this - status button; cell - selected instance actor; agent - selected task;
     * provider - task provider; codexInstanceCount - registered account count.
     * Returns: undefined.
     */
    _showTaskConfirmation(cell, agent, provider, codexInstanceCount) {
        this._hideTaskConfirmation();
        this._hideTaskHover();
        const content = new St.BoxLayout({vertical: true, reactive: true,
            style_class: 'ams-task-hover-content'});
        const details = this._agentMenuItem(agent, provider, codexInstanceCount, false);
        details.reactive = false;
        details.can_focus = false;
        content.add_child(details);
        content.add_child(new St.Label({text: 'Close this task and mark Done? Right-click to confirm · Esc to cancel',
            style_class: 'ams-task-confirmation-hint', reactive: false}));
        const popup = new BoxPointer.BoxPointer(St.Side.BOTTOM, {reactive: true});
        popup.add_style_class_name('popup-menu-boxpointer');
        popup.add_style_class_name('ams-task-hover');
        popup.bin.set_child(content);
        this._taskConfirmation = {cell, agent, popup};
        cell._amsConfirmation = () => this._confirmTaskCompletion();
        content.connect('button-press-event', (_actor, event) => {
            if (event.get_button() === 3)
                this._confirmTaskCompletion();
            return Clutter.EVENT_STOP;
        });
        this._taskConfirmationSignal = global.stage.connect('captured-event', (_stage, event) => {
            if (event.type() === Clutter.EventType.KEY_PRESS && event.get_key_symbol() === Clutter.KEY_Escape) {
                this._hideTaskConfirmation();
                return Clutter.EVENT_STOP;
            }
            if (event.type() === Clutter.EventType.BUTTON_PRESS) {
                const source = event.get_source();
                if (source !== cell && !cell.contains?.(source) && source !== popup && !popup.contains?.(source))
                    this._hideTaskConfirmation();
            }
            return Clutter.EVENT_PROPAGATE;
        });
        Main.layoutManager.addChrome(popup);
        popup.setPosition(cell, 0.5);
        popup.open(BoxPointer.PopupAnimation.NONE);
        // Grab the stage so both the popup and its original panel cell remain clickable.
        this._taskConfirmationGrab = Main.pushModal(global.stage, {actionMode: Shell.ActionMode.NORMAL});
    }

    /** Cancel an armed task and disconnect its stage input handler.
     * Parameters: this - status button owning the confirmation.
     * Returns: undefined.
     */
    _hideTaskConfirmation() {
        if (this._taskConfirmationSignal) {
            global.stage.disconnect(this._taskConfirmationSignal);
            this._taskConfirmationSignal = null;
        }
        if (this._taskConfirmationGrab) {
            Main.popModal(this._taskConfirmationGrab);
            this._taskConfirmationGrab = null;
        }
        const pending = this._taskConfirmation;
        this._taskConfirmation = null;
        if (pending) {
            pending.cell._amsConfirmation = null;
            pending.popup.destroy();
        }
    }

    /** Consume the selected task confirmation once before invoking completion.
     * Parameters: this - status button owning the confirmation.
     * Returns: undefined.
     */
    _confirmTaskCompletion() {
        const agent = this._taskConfirmation?.agent;
        this._hideTaskConfirmation();
        if (agent)
            this._completeAgent(agent);
    }

    /** Delegate the selected session to pa's authoritative ownership and completion service.
     * Parameters: this - status button; agent - confirmed task with its stable published session id.
     * Returns: undefined after scheduling completion, showing native failures without reporting success.
     */
    _completeAgent(agent) {
        try {
            const argv = completionCommand(agent);
            if (!argv)
                throw new Error('Selected window has no registered session id');
            const process = Gio.Subprocess.new(argv,
                Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE);
            process.communicate_utf8_async(null, null, (child, result) => {
                try {
                    const [ok, _output, stderr] = child.communicate_utf8_finish(result);
                    if (!ok || !child.get_successful()) {
                        const message = completionErrorMessage(stderr);
                        print(`acc-model-status: confirmed-close failed: ${message}`);
                        Main.notifyError('Could not mark task Done', message);
                    } else {
                        this._queueRefresh();
                    }
                } catch (error) {
                    print(`acc-model-status: confirmed-close failed: ${error.message}`);
                    Main.notifyError('Could not mark task Done', error.message);
                }
            });
        } catch (error) {
            print(`acc-model-status: confirmed-close rejected: ${error.message}`);
            Main.notifyError('Could not mark task Done', error.message);
        }
    }

    /**
     * Share one source for animation and completion ages.
     * Parameters: this - status button owning the bar actors and GLib source.
     * Returns: undefined.
     */
    _syncPanelPulseTimer() {
        print(`this=${this}`);
        const fresh = [...this._panelProgressBars.values()].filter(bar => !bar._amsStale);
        const animated = fresh.some(bar => bar._amsUnderway || bar._amsReady || bar._amsResetCountdown);
        const interval = animated ? 33 : fresh.some(bar => bar._amsCompletionAge) ? 1000 : 0;
        if (this._panelPulseId && interval !== this._panelPulseInterval) {
            GLib.source_remove(this._panelPulseId);
            this._panelPulseId = 0;
        }
        if (interval && !this._panelPulseId) {
            this._panelPulseInterval = interval;
            let lastAgeSecond;
            this._panelPulseId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, interval, () => {
                const now = Date.now();
                if (animated)
                    this._applyPanelProgressPulse(now);
                const second = Math.floor(now / 1000);
                if (second !== lastAgeSecond) {
                    for (const bar of this._panelProgressBars.values()) {
                        if (bar._amsCompletionAge && !bar._amsReady && !bar._amsStale)
                            bar._amsRuntimeLabel.text = panelAgentRuntimeText(bar._amsRuntimeAgent, now);
                    }
                    lastAgeSecond = second;
                }
                return GLib.SOURCE_CONTINUE;
            });
        }
        print(undefined);
    }

    /**
     * Update live clocks and unread completion ages with their bar pulses.
     * Parameters: this - status button; now - optional epoch milliseconds.
     * Returns: undefined.
     */
    _applyPanelProgressPulse(now = Date.now()) {
        for (const bar of this._panelProgressBars.values()) {
            if (bar._amsRuntimeLabel && (bar._amsUnderway || bar._amsReady || bar._amsResetCountdown) && !bar._amsStale) {
                bar._amsRuntimeLabel.text = panelAgentRuntimeText(bar._amsRuntimeAgent, now);
                bar._amsRuntimeLabel.visible = Boolean(bar._amsRuntimeLabel.text);
                bar._amsRuntimeLabel.style = `color: ${panelAgentRuntimeColor(bar._amsAgent, now)};`;
            }
            if ((bar._amsUnderway || bar._amsReady) && !bar._amsStale) {
                bar.opacity = panelProgressHostOpacity(bar._amsAgent, now, false, bar._amsReady);
                if (bar._amsReady || panelProgressGoalActive(bar._amsAgent)) {
                    bar._amsPulseAt = now;
                    bar.queue_repaint();
                }
            }
        }
    }

    /**
     * Keep a clickable announcement bell before providers in either mute state.
     * Parameters: this - status button; muted - effective published speech mute.
     * Returns: undefined.
     */
    _syncMuteIndicator(muted) {
        const iconName = muted
            ? 'notifications-disabled-symbolic' : 'preferences-system-notifications-symbolic';
        if (!this._muteIcon) {
            this._muteIcon = new St.Icon({
                icon_name: iconName,
                style_class: 'ams-icon',
                y_align: Clutter.ActorAlign.CENTER,
            });
            this._muteButton = new St.Button({
                style_class: 'ams-mute-button',
                can_focus: true,
                reactive: true,
                track_hover: true,
                y_align: Clutter.ActorAlign.CENTER,
            });
            this._muteButton.set_child(this._muteIcon);
            this._muteButton.connect('clicked', () => this._toggleAnnouncements());
            this._box.insert_child_at_index(this._muteButton, 0);
        }
        this._muteIcon.icon_name = iconName;
        this._muteIcon.style_class = muted ? 'ams-icon ams-muted' : 'ams-icon';
        this._muteButton.accessible_name = muted
            ? 'Unmute ACC announcements' : 'Mute ACC announcements';
        this._muteIcon.visible = true;
        this._muteButton.visible = true;
    }

    /**
     * Toggle shared announcement silence through the installed ACC dispatcher.
     * Parameters: this - status button requesting the toggle.
     * Returns: undefined.
     */
    _toggleAnnouncements() {
        const dispatcher = GLib.build_filenamev([
            GLib.get_home_dir(), '.local', 'bin', 'agent-command-center',
        ]);
        try {
            const process = Gio.Subprocess.new(
                [dispatcher, 'silent', 'toggle'], Gio.SubprocessFlags.NONE);
            process.wait_check_async(null, (child, result) => {
                try {
                    child.wait_check_finish(result);
                } catch (error) {
                    logError(error, 'acc-model-status: could not toggle announcements');
                }
            });
        } catch (error) {
            logError(error, 'acc-model-status: could not toggle announcements');
        }
    }

    /**
     * Tick live-run clocks once a second while any click-menu badge needs it.
     *
     * The payload carries ``run_started_at``; elapsed text is computed locally
     * from that instant so the menu matches ACC's top-right timer.
     */
    _syncRunClockTimer() {
        if (this._runClockActors.length === 0) {
            if (this._runClockId) {
                GLib.source_remove(this._runClockId);
                this._runClockId = 0;
            }
            return;
        }
        this._applyRunClocks();
        if (this._runClockId)
            return;
        this._runClockId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
            this._applyRunClocks();
            return GLib.SOURCE_CONTINUE;
        });
    }

    /** Recompute elapsed clock text on every live-run badge. */
    _applyRunClocks() {
        for (const badge of this._runClockActors) {
            if (badge._amsAgent)
                badge.text = agentPhaseBadgeText(badge._amsAgent);
        }
    }

    /** Render the "no agent terminals" placeholder (payload absent or empty). */
    _renderEmpty(payload) {
        const segment = new St.BoxLayout({style_class: 'ams-segment ams-empty'});
        segment.add_child(new St.Label({
            text: '0',
            style_class: 'ams-count',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        segment.add_child(new St.Icon({
            gicon: this._icon(FALLBACK_ICON),
            style_class: 'ams-icon system-status-icon',
        }));
        this._box.add_child(segment);
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
            payload ? 'No open agent terminals' : 'Agent Command Center not publishing',
            {reactive: false}));
    }

    /**
     * Render one provider segment: running count, icon, and tok/s when active.
     *
     * @param {object} provider - payload provider entry.
     * @param {boolean} stale - whether the whole payload is stale.
     * @param {object} payload - normalized payload with ``harnessRates``.
     */
    _renderProvider(provider, stale, payload) {
        const running = Number(provider.running) || 0;
        const attention = Number(provider.attention) || 0;
        let state = 'ams-idle';
        if (attention > 0)
            state = 'ams-attention';
        else if (running > 0)
            state = 'ams-active';

        const segment = new St.BoxLayout({
            style_class: `ams-segment ${state}${stale ? ' ams-stale' : ''}`,
        });
        segment.add_child(new St.Label({
            text: `${running}`,
            style_class: 'ams-count',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        segment.add_child(new St.Icon({
            gicon: this._icon(provider.icon),
            style_class: 'ams-icon system-status-icon',
        }));
        const rateRow = panelHarnessRateRows(payload).find(row => row.kind === provider.kind);
        const rateText = formatHarnessPanelRateText(rateRow);
        if (rateText) {
            segment.add_child(new St.Label({
                text: rateText,
                style_class: 'ams-rate',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }
        this._box.add_child(segment);
    }

    /**
     * Render panel tok/s chips for active harnesses not already in the bar.
     *
     * @param {object} payload - normalized payload with ``harnessRates``.
     * @param {boolean} stale - whether the payload is stale.
     * @param {Set<string>} shownKinds - provider kinds already drawn.
     */
    _renderPanelHarnessRates(payload, stale, shownKinds) {
        for (const row of panelHarnessRateRows(payload)) {
            if (shownKinds.has(row.kind))
                continue;
            const rateText = formatHarnessPanelRateText(row);
            if (!rateText)
                continue;
            const segment = new St.BoxLayout({
                style_class: `ams-segment ams-idle${stale ? ' ams-stale' : ''}`,
            });
            segment.add_child(new St.Icon({
                gicon: this._icon(row.icon || `acc-provider-${row.kind}-symbolic`),
                style_class: 'ams-icon system-status-icon',
            }));
            segment.add_child(new St.Label({
                text: rateText,
                style_class: 'ams-rate',
                y_align: Clutter.ActorAlign.CENTER,
            }));
            this._box.add_child(segment);
        }
    }

    /**
     * Render per-harness tok/s, tok/min, and tok/hour above workspace groups.
     *
     * @param {object} payload - normalized payload with ``harnessRates``.
     */
    _renderHarnessRates(payload) {
        const lines = harnessRateLines(payload);
        if (lines.length === 0)
            return;
        const section = new PopupMenu.PopupMenuSection();
        section.actor.add_style_class_name('ams-harness-rates');
        const heading = new PopupMenu.PopupMenuItem('Harness token rates', {reactive: false});
        heading.label.add_style_class_name('ams-menu-heading');
        section.addMenuItem(heading);
        for (const line of lines) {
            const item = new PopupMenu.PopupMenuItem(line, {reactive: false});
            item.label.add_style_class_name('ams-harness-rate');
            section.addMenuItem(item);
        }
        this.menu.addMenuItem(section);
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
    }

    /**
     * Build workspace sections with ID-ordered rows and monitor separators.
     * Parameters: this - model-status button; payload - normalized task rows;
     *     stale - whether data is stale; attention - workspace waiting counts.
     * Returns: undefined.
     *
     * @param {object} payload - normalized payload.
     * @param {boolean} stale - whether the payload is stale.
     * @param {Array<object>} attention - per-workspace waiting counts from
     *     {@link workspaceAttentionCounts}, repeated as badges in the headings.
     */
    _renderMenu(payload, stale, attention) {
        const workspaceEntries = [];
        for (const provider of payload.providers) {
            const agents = Array.isArray(provider.agents) ? provider.agents : [];
            for (const agent of agents)
                workspaceEntries.push({provider, agent});
        }
        const workspaceGroups = groupWorkspaceEntries(workspaceEntries);
        // Unread counts keyed for the workspace headings in the drop-down.
        const attentionByWorkspace = new Map(
            (attention || []).map(entry => [entry.workspace, entry]));

        let workspaceSection = null;
        for (let groupIndex = 0; groupIndex < workspaceGroups.length; groupIndex++) {
            const group = workspaceGroups[groupIndex];
            const workspace = group.workspace;
            if (groupIndex > 0)
                this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
            const current = group.entries.some(entry =>
                Boolean(entry.agent.current_workspace));
            const headingText = workspace > 0
                ? `Workspace ${workspace}${current ? ' — current' : ''}`
                : 'Workspace unknown';
            const heading = new PopupMenu.PopupMenuItem(headingText, {reactive: false});
            heading.label.add_style_class_name('ams-menu-heading');
            const waiting = attentionByWorkspace.get(workspace);
            if (waiting) {
                // Show unread workspace counts only inside the drop-down.
                const badge = new St.Label({
                    text: waiting.label,
                    style_class: 'ams-ws-attn',
                    y_align: Clutter.ActorAlign.CENTER,
                });
                heading.add_child(badge);
            }
            workspaceSection = new PopupMenu.PopupMenuSection();
            workspaceSection.actor.add_style_class_name('ams-workspace-block');
            if (current)
                heading.actor.add_style_class_name('ams-current-workspace-block');
            workspaceSection.addMenuItem(heading);
            this.menu.addMenuItem(workspaceSection);
            let previousMonitor = null;
            for (const entry of group.entries) {
                const monitor = monitorSortValue(entry.agent.monitor);
                if (previousMonitor !== null && monitor !== previousMonitor) {
                    workspaceSection.addMenuItem(buildMonitorSeparator());
                }
                workspaceSection.addMenuItem(this._agentMenuItem(entry.agent, entry.provider, payload.codexInstanceCount));
                previousMonitor = monitor;
            }
        }

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const footer = stale
            ? `Stale — last update ${payload.updatedAt || 'unknown'}`
            : `${payload.totalRunning} running / ${payload.totalLive} live`;
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(footer, {reactive: false}));
    }

    /** Build the colored detail row for the menu or a noninteractive hover preview.
     * Parameters: this - status button; agent - window-scoped task; provider - owning provider;
     * codexInstanceCount - registered accounts; track - whether to register menu clocks and progress.
     * Returns: the task menu item with its colored detail row.
     */
    _agentMenuItem(agent, provider, codexInstanceCount, track = true) {
        print(`this=${this} agent=${agent} provider=${provider} codexInstanceCount=${codexInstanceCount} track=${track}`);
        const noteId = String(agent.note_id || '').trim();
        const actionable = Boolean(noteId);
        const item = new PopupMenu.PopupBaseMenuItem({
            reactive: actionable,
            can_focus: actionable,
        });
        if (actionable) {
            item.connect('activate', (_item, event) => {
                if (item._amsSuppressActivation) {
                    item._amsSuppressActivation = false;
                    if (this._taskConfirmation?.agent.session_id === agent.session_id)
                        return;
                }
                if (track && event?.get_button?.() === 1 &&
                    (event.get_state() & Clutter.ModifierType.CONTROL_MASK)) {
                    this._armMenuTaskConfirmation(agent, provider, codexInstanceCount);
                    return;
                }
                this._focusAgent(noteId);
            });
            if (track) {
                item.connect('captured-event', (_item, event) => {
                    if (event.type() !== Clutter.EventType.BUTTON_PRESS || event.get_button() !== 1 || !(event.get_state() & Clutter.ModifierType.CONTROL_MASK))
                        return Clutter.EVENT_PROPAGATE;
                    item._amsSuppressActivation = true;
                    this._armMenuTaskConfirmation(agent, provider, codexInstanceCount);
                    return Clutter.EVENT_STOP;
                });
            }
        }
        const row = new St.BoxLayout({style_class: 'ams-agent-row'});

        const projectTag = String(agent.project_tag || '').trim();
        if (projectTag) {
            row.add_child(new St.Label({
                text: projectTag,
                style_class: 'ams-project-tag',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        const isVibe = (agent.kind || provider.kind) === 'vibe';
        const icon = isVibe
            ? new Gio.FileIcon({file: this._iconsDir.get_child('mistral.svg')})
            : this._icon(provider.icon);
        row.add_child(new St.Icon({
            gicon: icon,
            style_class: 'ams-menu-provider-icon system-status-icon',
            accessible_name: `${provider.label || provider.kind || 'Agent'}`,
            y_align: Clutter.ActorAlign.CENTER,
        }));

        const instanceLabel = codexInstanceLabel(agent, codexInstanceCount);
        if (instanceLabel) {
            const tag = new St.Label({
                text: instanceLabel[0],
                style_class: 'ams-instance-tag',
                y_align: Clutter.ActorAlign.CENTER,
            });
            tag.set_style(`color: ${instanceLabel[1]};`);
            row.add_child(tag);
        }

        const badge = new St.Label({
            text: agentPhaseBadgeText(agent),
            style_class: 'ams-phase',
            y_align: Clutter.ActorAlign.CENTER,
        });
        badge.set_style(`color: ${agentPhaseBadgeColor(agent)};`);
        if (track && ((agent.run_active && String(agent.phase || '').trim().toUpperCase() === 'RUNNING') ||
            agent.queued_reset_at)) {
            badge._amsAgent = agent;
            this._runClockActors.push(badge);
        }
        row.add_child(badge);

        if (agent.title) {
            row.add_child(new St.Label({
                text: `${agent.title}`,
                style_class: 'ams-agent-title',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        const origin = [];
        if (agent.branch)
            origin.push(`(${agent.branch})`);
        if (origin.length > 0) {
            row.add_child(new St.Label({
                text: origin.join(' '),
                style_class: 'ams-agent-origin',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        const estimateLabel = new St.Label({
            text: taskEstimateText(agent),
            style_class: 'ams-agent-estimate',
            x_expand: true,
            x_align: Clutter.ActorAlign.END,
            y_align: Clutter.ActorAlign.CENTER,
        });
        estimateLabel.visible = Boolean(estimateLabel.text);
        row.add_child(estimateLabel);
        const content = new St.BoxLayout({vertical: true, x_expand: true});
        const contextLabel = new St.Label({text: '', visible: false,
            style_class: 'ams-agent-context', x_expand: true, width: 0});
        contextLabel.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        content.add_child(row);
        content.add_child(contextLabel);
        const widgets = buildTaskProgressLayer(item, content, badge, agent);
        widgets.estimateLabel = estimateLabel;
        widgets.contextLabel = contextLabel;
        widgets.compactContext = !track;
        updateTaskProgress(widgets, agent);
        const key = String(agent.session_id || agent.tmux_name || agent.key || '');
        if (track && key)
            this._taskRows.set(key, widgets);
        print(item);
        return item;
    }

    /** Hide a task and repaint using the latest panel snapshot.
     * Parameters: this - status button; key - selected task key.
     * Returns: undefined.
     */
    _hidePanelTask(key) {
        print(`this=${this} key=${key}`);
        this._hiddenPanelTasks.add(key);
        this._updatePanelProgress(this._panelProgressPayload, this._panelProgressStale);
        print(undefined);
    }

    /** Anchor a menu-requested confirmation to the task's current panel cell.
     * Parameters: this - status button; agent - selected menu task; provider - row provider;
     * codexInstanceCount - account count for the detail colors.
     * Returns: undefined; vanished panel targets remain unarmed.
     */
    _armMenuTaskConfirmation(agent, provider, codexInstanceCount) {
        const key = String(agent.session_id || agent.tmux_name || agent.key || '');
        const bar = this._panelProgressBars?.get(key);
        if (bar) {
            this.menu.close();
            this._showTaskConfirmation(bar.get_parent(), bar._amsAgent || agent,
                bar._amsProvider || provider, bar._amsInstanceCount || codexInstanceCount);
        }
    }

    /** Activate one agent terminal through ACC's shared focus-and-pointer selector. */
    _focusAgent(noteId) {
        const targetNoteId = String(noteId || '').trim();
        if (!targetNoteId) {
            log('acc-model-status: cannot focus an agent without a note id');
            return;
        }
        this.menu.close();
        const selectorPath = GLib.build_filenamev([
            GLib.get_home_dir(), '.local', 'bin', 'acc-window-select',
        ]);
        try {
            const process = Gio.Subprocess.new(
                [selectorPath, '--note-id', targetNoteId],
                Gio.SubprocessFlags.NONE,
            );
            process.wait_check_async(null, (child, result) => {
                try {
                    child.wait_check_finish(result);
                } catch (error) {
                    logError(error, 'acc-model-status: agent terminal focus failed');
                }
            });
        } catch (error) {
            logError(error, 'acc-model-status: could not launch agent terminal selector');
        }
    }

    /** Tear down timers and the file monitor before the actor goes away.
     * Parameters: this - status button being destroyed.
     * Returns: undefined.
     */
    destroy() {
        this._hideTaskHover();
        this._hideTaskConfirmation();
        if (this._panelPulseId) {
            GLib.source_remove(this._panelPulseId);
            this._panelPulseId = 0;
        }
        if (this._refreshId) {
            GLib.source_remove(this._refreshId);
            this._refreshId = 0;
        }
        if (this._pollId) {
            GLib.source_remove(this._pollId);
            this._pollId = 0;
        }
        if (this._runClockId) {
            GLib.source_remove(this._runClockId);
            this._runClockId = 0;
        }
        this._runClockActors = [];
        this._taskRows.clear();
        if (this._monitor) {
            this._monitor.cancel();
            this._monitor = null;
        }
        for (const [object, id] of this._shellSignals)
            object.disconnect(id);
        this._shellSignals = [];
        for (const [win, ids] of this._windowSignals) {
            for (const id of ids)
                win.disconnect(id);
        }
        this._windowSignals.clear();
        super.destroy();
    }
});

// Install the callback before GObject invokes _init; class fields initialize too late.
/**
 * Refresh progress bars, runtime clocks and running forecasts while retaining stable actors.
 * Parameters: this - status button with independent Harbor entries; payload - scoped agent snapshot or null;
 * stale - whether ACC metadata is old; fresh Harbor entries retain their own liveness.
 * Returns: undefined after refreshing all tasks in their shared workspace groups.
 */
AccModelStatusButton.prototype._updatePanelProgress = function (payload, stale) {
    print(`this=${this} payload=${payload} stale=${stale}`);
    this._panelProgressPayload = payload;
    this._panelProgressStale = stale;
    if (!this._panelProgress) {
        this._hiddenPanelTasks = new Set();
        this._panelProgress = new St.BoxLayout({style_class: 'ams-panel-progress'});
        this._panelProgressBars = new Map();
        this._panelProgressKey = '';
    }
    if (this._panelProgress.get_parent() !== this._box)
        this._box.add_child(this._panelProgress);
    else if (this._box.get_children().at(-1) !== this._panelProgress) {
        // Move inert summaries around the pointer target, never restack it.
        for (const child of this._box.get_children()) {
            if (child !== this._panelProgress)
                this._box.set_child_below_sibling(child, this._panelProgress);
        }
    }
    const entries = (payload?.providers || []).flatMap(provider =>
        (provider.agents || []).map((agent, index) => ({provider, agent,
            key: String(agent.session_id || agent.tmux_name || agent.key || `${provider.kind}:${index}`)}))).concat(this._harborEntries || []);
    const groups = groupWorkspaceEntries(entries);
    const hidden = this._hiddenPanelTasks;
    const key = JSON.stringify(groups.map(group => [group.workspace, group.entries.map(entry =>
        [entry.key, monitorSortValue(entry.agent.monitor), hidden.has(entry.key)])]));
    if (key !== this._panelProgressKey) {
        this._hideTaskHover();
        this._panelProgress.destroy_all_children();
        this._panelProgressBars.clear();
        for (const [index, group] of groups.entries()) {
            if (index > 0)
                this._panelProgress.add_child(buildPanelWorkspaceSeparator());
            const groupActor = new St.BoxLayout({
                style_class: 'ams-panel-workspace-group',
                reactive: false,
            });
            groupActor._amsWorkspace = group.workspace;
            const box = new St.BoxLayout({style_class: 'ams-panel-workspace',
                y_expand: true, y_align: Clutter.ActorAlign.FILL});
            let previousMonitor = null;
            for (const entry of group.entries) {
                if (hidden.has(entry.key))
                    continue;
                const monitor = monitorSortValue(entry.agent.monitor);
                if (previousMonitor !== null && monitor !== previousMonitor)
                    box.add_child(buildPanelMonitorSeparator());
                previousMonitor = monitor;
                const cell = buildPanelTaskCell(entry.agent, noteId => this._focusAgent(noteId),
                    () => this._hidePanelTask(entry.key));
                const bar = cell._amsBar;
                cell.connect('notify::hover', () => this._syncTaskHover(
                    cell, bar._amsAgent, bar._amsProvider, bar._amsInstanceCount));
                cell.connect('destroy', () => {
                    if (this._taskHoverCell === cell)
                        this._hideTaskHover();
                    if (this._taskConfirmation?.cell === cell)
                        this._hideTaskConfirmation();
                });
                this._panelProgressBars.set(entry.key, bar);
                box.add_child(cell);
            }
            const label = new St.Label({
                style_class: 'ams-panel-workspace-label',
                style: `color: ${panelWorkspaceColor(group.workspace)};`,
                text: group.workspace > 0 ? `W${group.workspace}` : 'W?',
                x_align: Clutter.ActorAlign.START,
                y_align: Clutter.ActorAlign.CENTER,
                reactive: false,
            });
            groupActor.add_child(label);
            groupActor.add_child(box);
            this._panelProgress.add_child(groupActor);
        }
        this._panelProgressKey = key;
    }
    const instanceNumbers = panelInstanceNumbers(entries, payload?.codexInstanceCount);
    const showCodexAccounts = payload?.codexInstanceCount > 1;
    let exhaustedAccounts = new Set();
    if (showCodexAccounts && entries.some(({agent, provider}) => (agent.kind || provider.kind) === 'codex')) {
        this._codexLimitFile ??= Gio.File.new_for_path(
            GLib.build_filenamev([GLib.get_user_state_dir(), 'agentic-cli-status', 'status.json']));
        exhaustedAccounts = exhaustedCodexAccounts(readPanelQuotaSnapshot(this._codexLimitFile), Date.now() / 1000);
    }
    for (const entry of entries) {
        const entryStale = entry.agent.harbor ? false : stale;
        const bar = this._panelProgressBars.get(entry.key);
        if (!bar)
            continue;
        bar._amsAgent = entry.agent;
        bar._amsProvider = entry.provider;
        bar._amsInstanceCount = payload?.codexInstanceCount;
        const cell = bar.get_parent();
        const kind = entry.agent.kind || entry.provider.kind;
        const publishedIcon = kind === entry.provider.kind ? entry.provider.icon : null;
        const icon = publishedIcon && publishedIcon !== FALLBACK_ICON ? publishedIcon :
            kind ? `acc-provider-${kind}-symbolic` : FALLBACK_ICON;
        if (bar._amsLogoName !== icon) {
            bar._amsLogo.gicon = this._icon(icon);
            bar._amsLogoName = icon;
        }
        bar._amsLogo.visible = true;
        bar._amsLogo.opacity = entryStale ? 31 : 62;
        cell.can_focus = Boolean(String(entry.agent.note_id || '').trim());
        cell.track_hover = cell.can_focus;
        const estimate = bar._amsEstimateLabel;
        estimate.text = panelAgentEstimateText(entry.agent);
        estimate.visible = Boolean(estimate.text);
        estimate.opacity = entryStale ? 128 : 255;
        const instance = bar._amsInstanceLabel;
        const instanceNumber = instanceNumbers.get(entry.key);
        instance.text = String(instanceNumber ?? '');
        instance.style = showCodexAccounts && kind === 'codex' && exhaustedAccounts.has(instanceNumber)
            ? 'color: #ef4444;' : null;
        instance.visible = Boolean(instance.text);
        instance.opacity = estimate.opacity;
        const now = Date.now();
        const publishedAt = Date.parse(payload?.updatedAt || '');
        const runtimeAt = entryStale ? (Number.isFinite(publishedAt) ? publishedAt : bar._amsRuntimeAt ?? now) : now;
        bar._amsRuntimeAt = runtimeAt;
        bar._amsRuntimeAgent = entry.agent;
        bar._amsResetCountdown = panelLaunchWaitSeconds(entry.agent, runtimeAt) !== null;
        const runtime = bar._amsRuntimeLabel;
        runtime.text = panelAgentRuntimeText(entry.agent, runtimeAt);
        bar._amsCompletionAge = runtime.text.endsWith(' ago');
        runtime.style_class = bar._amsCompletionAge ? 'ams-panel-agent-runtime ams-panel-agent-age' : 'ams-panel-agent-runtime';
        runtime.style = `color: ${panelAgentRuntimeColor(entry.agent, now, entryStale)};`;
        runtime.visible = Boolean(runtime.text);
        runtime.opacity = entryStale ? 128 : 255;
        bar._amsUnderway = panelProgressUnderway(entry.agent);
        bar._amsReady = entry.agent.unread_completion === true;
        bar._amsStale = entryStale;
        bar._amsPulseAt = now;
        bar.opacity = panelProgressHostOpacity(entry.agent, bar._amsPulseAt, entryStale, bar._amsReady);
        bar.queue_repaint();
        if (cell.hover)
            this._syncTaskHover(cell, entry.agent, entry.provider, payload?.codexInstanceCount);
    }
    this._panelProgress.visible = entries.length > 0;
    this._syncPanelPulseTimer();
    print(undefined);
};


export default class AccModelStatusExtension extends Extension {
    /** Add agent and independent Harbor indicators to the panel.
     * Parameters: this - owning extension.
     * Returns: undefined.
     */
    enable() {
        print(`this=${this}`);
        this._button = new AccModelStatusButton(this);
        Main.panel.addToStatusArea('acc-model-status', this._button, 0, 'right');
        this._harborStop = attachHarborStatus(this._button);
        try {
            this._switchGroupSettings = new Gio.Settings({schema_id: SWITCH_GROUP_SCHEMA});
            this._shortcutBackupFile = Gio.File.new_for_path(GLib.build_filenamev([
                GLib.get_user_state_dir(), 'acc-model-status', MENU_ACCELERATOR_BACKUP_FILENAME,
            ]));
        } catch (error) {
            logError(error, 'acc-model-status: could not access GNOME keybindings');
            print(undefined);
            return;
        }
        if (!this._reserveMenuAccelerator()) {
            print(undefined);
            return;
        }

        try {
            this._menuAcceleratorSettings = this.getSettings();
            Main.wm.addKeybinding(
                'menu-accelerator',
                this._menuAcceleratorSettings,
                Meta.KeyBindingFlags.NONE,
                Shell.ActionMode.ALL,
                () => {
                    if (this._button) {
                        this._button.menu.toggle();
                        log('acc-model-status: toggled menu with Alt+§');
                    }
                },
            );
            this._menuAcceleratorRegistered = true;
            log('acc-model-status: registered Alt+§ menu shortcut');
        } catch (error) {
            logError(error, 'acc-model-status: could not register Alt+§ menu shortcut');
            this._restoreMenuAccelerator();
        }
        print(undefined);
    }

    /** Remove both indicators and every resource they own.
     * Parameters: this - owning extension.
     * Returns: undefined.
     */
    disable() {
        print(`this=${this}`);
        this._harborStop?.();
        this._harborStop = null;
        if (this._menuAcceleratorRegistered) {
            Main.wm.removeKeybinding('menu-accelerator');
            this._menuAcceleratorRegistered = false;
        }
        this._menuAcceleratorSettings = null;
        this._restoreMenuAccelerator();
        this._button?.destroy();
        this._button = null;
        print(undefined);
    }

    /** Reserve the accelerator and keep a restart-safe backup of displaced settings. */
    _reserveMenuAccelerator() {
        const backup = readMenuShortcutBackup(this._shortcutBackupFile);
        if (backup === null)
            return false;
        const current = this._switchGroupSettings.get_strv(SWITCH_GROUP_KEY);
        const collisions = agentStatusMenuShortcutBindings(current);
        const saved = restoreAgentStatusMenuShortcuts(backup, collisions);
        if (saved.length > 0 && !saveMenuShortcutBackup(this._shortcutBackupFile, saved))
            return false;

        const reserved = withoutAgentStatusMenuShortcuts(current);
        if (reserved.length !== current.length &&
            !this._switchGroupSettings.set_strv(SWITCH_GROUP_KEY, reserved)) {
            log('acc-model-status: unable to reserve Alt+§ in GNOME keybindings');
            return false;
        }
        if (reserved.length !== current.length)
            log('acc-model-status: reserved Alt+§ for the agent status menu');
        return true;
    }

    /** Restore displaced GNOME shortcuts when the menu extension is disabled. */
    _restoreMenuAccelerator() {
        if (!this._switchGroupSettings || !this._shortcutBackupFile)
            return;
        const backup = readMenuShortcutBackup(this._shortcutBackupFile);
        if (backup === null || backup.length === 0)
            return;
        const current = this._switchGroupSettings.get_strv(SWITCH_GROUP_KEY);
        const restored = restoreAgentStatusMenuShortcuts(current, backup);
        if (restored.length !== current.length &&
            !this._switchGroupSettings.set_strv(SWITCH_GROUP_KEY, restored)) {
            log('acc-model-status: unable to restore GNOME group-switch shortcuts');
            return;
        }
        deleteMenuShortcutBackup(this._shortcutBackupFile);
        if (restored.length !== current.length)
            log('acc-model-status: restored GNOME group-switch shortcuts');
    }
}
