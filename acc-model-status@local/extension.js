/* extension.js — acc-model-status@local
 *
 * Optional GNOME Shell taskbar indicator for Agent Command Center. It renders
 * one icon per agent provider (Claude, Codex, Grok, Qwen, …) together with the
 * number of agents of that provider that are currently RUNNING.
 *
 * Separation of concerns (same contract as agentic-cli-status@local):
 *   - Agent Command Center's monitor OWNS the data: it publishes the document
 *     `model_status_bar.json` into the tracker state directory.
 *   - This extension joins that document to GNOME Shell's live window metadata
 *     so only agents with an open terminal window are rendered, across every
 *     workspace. It never scans tmux, spawns a subprocess, or talks to the
 *     network; it watches the file and Shell window/workspace signals directly.
 *
 * Components:
 *   AccModelStatusButton  panel button: segment box + agent list menu
 *   payloadFile()         resolves the published document's path
 *   readPayload()         parses/validates the document, tolerating partial writes
 */

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

/** Document published by the Agent Command Center monitor. */
const PAYLOAD_FILENAME = 'model_status_bar.json';
/** Tracker state directory relative to the user's state home. */
const STATE_DIR_NAME = 'codex-agent-tracker';
/** Environment override honored by the tracker (and therefore by us). */
const STATE_DIR_ENV = 'CODEX_AGENT_TRACKER_STATE_DIR';
/** Safety-net re-read interval (seconds); the file monitor does the real work. */
const POLL_SECONDS = 15;
/** Payload older than this is rendered dimmed — the monitor is not publishing. */
const STALE_SECONDS = 180;
/** Fallback icon for providers this extension ships no artwork for. */
const FALLBACK_ICON = 'acc-provider-generic-symbolic';
/** Phases contributing to the scoped provider counters. */
const RUNNING_PHASES = new Set(['RUNNING']);
const ATTENTION_PHASES = new Set(['WAITING', 'INTERRUPTED']);
/**
 * Phases meaning the agent is waiting on the user: it asked a question
 * (ATTN/WAITING), its run was interrupted or stopped, or it finished its turn
 * (DONE). These raise the per-workspace badges; RUNNING and DRAFT never do.
 */
const NEEDS_USER_PHASES = new Set(['ATTN', 'WAITING', 'INTERRUPTED', 'STOPPED', 'DONE']);
/** Flash half-period of the per-workspace attention badges (milliseconds). */
const FLASH_INTERVAL_MS = 650;
/**
 * Agent phase palette — the SAME colors the phrase automation notes menu uses
 * for its agent badges (AGENT_PHASE_BADGE_COLORS in
 * src/overlay/notes_display.py). Keep the two in sync: a status looks the same
 * whether you read it in the notes overlay or in this panel menu.
 */
const PHASE_COLORS = {
    RUNNING: '#e6c200',
    ATTN: '#ffb454',
    WAITING: '#ffb454',
    DONE: '#22c55e',
    DRAFT: '#84c32f',
    STOPPED: '#ef4444',
    INTERRUPTED: '#c084fc',
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
 * A live run with a known start shows the compact yellow clock instead of the
 * word ``RUNNING``. Other phases keep their phase word (``DONE``, ``ATTN``, …).
 *
 * @param {object} agent - payload agent row.
 * @param {number} [nowMs] - clock override in milliseconds (tests).
 * @returns {string} clock text or uppercase phase word.
 */
function agentPhaseBadgeText(agent, nowMs) {
    const phase = String(agent && agent.phase || 'UNKNOWN').trim().toUpperCase();
    const started = Date.parse(agent && agent.run_started_at || '');
    const active = Boolean(agent && agent.run_active);
    if (phase === 'RUNNING' && active && !Number.isNaN(started)) {
        const now = Number.isFinite(nowMs) ? nowMs : Date.now();
        return formatRunTimer((now - started) / 1000);
    }
    return phase;
}

/**
 * Color for one click-menu agent badge.
 *
 * Live-run clocks use RUNNING yellow even though the visible text is the
 * elapsed clock rather than the word ``RUNNING``.
 *
 * @param {object} agent - payload agent row.
 * @returns {string} CSS color from {@link PHASE_COLORS}.
 */
function agentPhaseBadgeColor(agent) {
    const phase = String(agent && agent.phase || '').trim().toUpperCase();
    if (phase === 'RUNNING')
        return PHASE_COLORS.RUNNING;
    return phaseColor(phase);
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
 * @param {Gio.File} file - payload location.
 * @returns {object|null} normalized payload, or null when absent/unreadable/
 *     mid-write (the monitor writes atomically, but a truncated read still
 *     must never throw into the Shell).
 */
function readPayload(file) {
    let bytes;
    try {
        const [ok, contents] = file.load_contents(null);
        if (!ok)
            return null;
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
    if (!parsed || typeof parsed !== 'object')
        return null;

    const providers = Array.isArray(parsed.providers) ? parsed.providers : [];
    return {
        providers: providers.filter(entry => entry && typeof entry === 'object'),
        totalLive: Number(parsed.total_live) || 0,
        totalRunning: Number(parsed.total_running) || 0,
        updatedAt: typeof parsed.updated_at === 'string' ? parsed.updated_at : '',
    };
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

/** Sort workspace-menu entries by monitor, workspace, provider, and title. */
function compareWorkspaceEntries(left, right) {
    const monitorDifference = monitorSortValue(left.agent.monitor) -
        monitorSortValue(right.agent.monitor);
    if (monitorDifference !== 0)
        return monitorDifference;
    const workspaceDifference = workspaceSortValue(left.agent.workspace) -
        workspaceSortValue(right.agent.workspace);
    if (workspaceDifference !== 0)
        return workspaceDifference;
    const providerDifference = String(left.provider.label || '').localeCompare(
        String(right.provider.label || ''));
    if (providerDifference !== 0)
        return providerDifference;
    return String(left.agent.title || '').localeCompare(String(right.agent.title || ''));
}

/** Group once per workspace, ordering groups and rows by their first monitor. */
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
    orderedGroups.sort((left, right) => compareWorkspaceEntries(
        left.entries[0], right.entries[0]));
    return orderedGroups;
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
 * Count the agents waiting on the user, grouped by workspace.
 *
 * Drives the flashing `W<n>:<count>` badges in the panel and the matching
 * badges in the drop-down's workspace headings, so the bar answers "where do
 * I have to go?" without opening the menu.
 *
 * @param {object} payload - payload already scoped to live windows, so every
 *     agent carries the `workspace`/`current_workspace` of its own window.
 * @returns {Array<{workspace: number, count: number, current: boolean,
 *     label: string}>} one entry per workspace with at least one waiting
 *     agent, in ascending workspace order. Agents whose window could not be
 *     resolved (workspace 0) are skipped — there is nowhere to switch to.
 */
function workspaceAttentionCounts(payload) {
    const providers = Array.isArray(payload?.providers) ? payload.providers : [];
    const byWorkspace = new Map();
    for (const provider of providers) {
        const agents = Array.isArray(provider.agents) ? provider.agents : [];
        for (const agent of agents) {
            const phase = String(agent.phase || '').trim().toUpperCase();
            if (!NEEDS_USER_PHASES.has(phase))
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

const AccModelStatusButton = GObject.registerClass(
class AccModelStatusButton extends PanelMenu.Button {
    /**
     * Build the panel button.
     *
     * @param {Extension} extension - owning extension (for the icon directory).
     */
    _init(extension) {
        super._init(0.5, 'ACC Model Status');
        this._iconsDir = extension.dir.get_child('icons');
        this._file = payloadFile();
        this._monitor = null;
        this._pollId = 0;
        this._refreshId = 0;
        this._flashId = 0;
        this._runClockId = 0;
        this._flashOn = true;
        this._flashActors = [];
        this._runClockActors = [];
        this._shellSignals = [];
        this._windowSignals = new Map();

        this._box = new St.BoxLayout({style_class: 'ams-box panel-status-menu-box'});
        this.add_child(this._box);

        this._startWatching();
        this._startWindowWatching();
        this._refresh();
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
            windows.push({title, monitor, workspace, current});
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

    /** Re-read the payload and repaint the panel segments and the menu. */
    _refresh() {
        const published = readPayload(this._file);
        const windows = this._openWindowDescriptors();
        const payload = published
            ? scopePayloadToWindows(published, windows)
            : null;
        this._box.destroy_all_children();
        this.menu.removeAll();
        // Actors are re-collected on every repaint; never keep stale ones.
        this._flashActors = [];
        this._runClockActors = [];

        if (!payload || payload.providers.length === 0) {
            this._renderEmpty(payload);
            this._syncFlashTimer();
            this._syncRunClockTimer();
            return;
        }

        const stale = ageSeconds(payload.updatedAt) > STALE_SECONDS;
        // Counted once and shared, so the bar and the drop-down never disagree.
        const attention = workspaceAttentionCounts(payload);
        for (const provider of payload.providers)
            this._renderProvider(provider, stale);
        this._renderWorkspaceAttention(attention, stale);
        this._renderMenu(payload, stale, attention);
        this._syncFlashTimer();
        this._syncRunClockTimer();
    }

    /**
     * Render the flashing per-workspace attention badges at the right end of
     * the panel, e.g. `W4:1` when workspace 4 holds one agent that needs the
     * user. Clicking the bar still opens the menu, which repeats the badges.
     *
     * @param {Array<object>} entries - result of {@link workspaceAttentionCounts}.
     * @param {boolean} stale - whether the payload is stale (dims the badges).
     */
    _renderWorkspaceAttention(entries, stale) {
        for (const entry of entries) {
            const badge = new St.Label({
                text: entry.label,
                style_class: entry.current
                    ? 'ams-ws-attn ams-ws-attn-current'
                    : 'ams-ws-attn',
                y_align: Clutter.ActorAlign.CENTER,
            });
            if (stale)
                badge.add_style_class_name('ams-stale');
            this._box.add_child(badge);
            this._flashActors.push(badge);
        }
    }

    /**
     * Start or stop the flash timer so it only ticks while badges exist.
     *
     * Badges alternate between full and dim opacity every
     * {@link FLASH_INTERVAL_MS}; the timer is removed as soon as the last
     * badge disappears so an idle panel costs nothing.
     */
    _syncFlashTimer() {
        if (this._flashActors.length === 0) {
            if (this._flashId) {
                GLib.source_remove(this._flashId);
                this._flashId = 0;
            }
            this._flashOn = true;
            return;
        }
        // A fresh repaint must not inherit a half-faded state.
        this._applyFlashState();
        if (this._flashId)
            return;
        this._flashId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, FLASH_INTERVAL_MS, () => {
            this._flashOn = !this._flashOn;
            this._applyFlashState();
            return GLib.SOURCE_CONTINUE;
        });
    }

    /** Push the current flash phase onto every live badge actor. */
    _applyFlashState() {
        for (const actor of this._flashActors) {
            if (this._flashOn)
                actor.remove_style_class_name('ams-ws-attn-dim');
            else
                actor.add_style_class_name('ams-ws-attn-dim');
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
        segment.add_child(new St.Icon({
            gicon: this._icon(FALLBACK_ICON),
            style_class: 'ams-icon system-status-icon',
        }));
        segment.add_child(new St.Label({
            text: '0',
            style_class: 'ams-count',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        this._box.add_child(segment);
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
            payload ? 'No open agent terminals' : 'Agent Command Center not publishing',
            {reactive: false}));
    }

    /**
     * Render one provider segment: icon plus its running-agent count.
     *
     * @param {object} provider - payload provider entry.
     * @param {boolean} stale - whether the whole payload is stale.
     */
    _renderProvider(provider, stale) {
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
        segment.add_child(new St.Icon({
            gicon: this._icon(provider.icon),
            style_class: 'ams-icon system-status-icon',
        }));
        segment.add_child(new St.Label({
            text: `${running}`,
            style_class: 'ams-count',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        this._box.add_child(segment);
    }

    /**
     * Build one menu section per workspace, with monitor-ordered sections and
     * entries.
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
        // Same counts the panel badges show, keyed for the workspace headings.
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
                // Mirror the flashing panel badge inside the drop-down.
                const badge = new St.Label({
                    text: waiting.label,
                    style_class: 'ams-ws-attn',
                    y_align: Clutter.ActorAlign.CENTER,
                });
                heading.add_child(badge);
                this._flashActors.push(badge);
            }
            workspaceSection = new PopupMenu.PopupMenuSection();
            workspaceSection.actor.add_style_class_name('ams-workspace-block');
            if (current)
                workspaceSection.actor.add_style_class_name('ams-current-workspace-block');
            workspaceSection.addMenuItem(heading);
            this.menu.addMenuItem(workspaceSection);
            for (const entry of group.entries)
                workspaceSection.addMenuItem(this._agentMenuItem(entry.agent, entry.provider));
        }

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const footer = stale
            ? `Stale — last update ${payload.updatedAt || 'unknown'}`
            : `${payload.totalRunning} running / ${payload.totalLive} live`;
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(footer, {reactive: false}));
    }

    /**
     * Build one agent row: project tag, colored phase badge, title, origin.
     *
     * The leading tag is Phrase Automation's persisted compact project label
     * (for example ``INS`` for installation_scripts), omitted when ACC could
     * not match the agent path to a project. Workspace numbers stay in the
     * group headings. A live run shows the compact ACC clock in RUNNING
     * yellow instead of the word RUNNING; other phases keep their word.
     *
     * @param {object} agent - agent entry enriched from its live window.
     * @param {object} provider - provider metadata owning the agent.
     * @returns {PopupMenu.PopupBaseMenuItem} the non-reactive row.
     */
    _agentMenuItem(agent, provider) {
        const item = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
        });
        const row = new St.BoxLayout({style_class: 'ams-agent-row'});

        const projectTag = String(agent.project_tag || '').trim();
        if (projectTag) {
            row.add_child(new St.Label({
                text: projectTag,
                style_class: 'ams-project-tag',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        row.add_child(new St.Icon({
            gicon: this._icon(provider.icon),
            style_class: 'ams-menu-provider-icon system-status-icon',
            accessible_name: `${provider.label || provider.kind || 'Agent'}`,
            y_align: Clutter.ActorAlign.CENTER,
        }));

        const badge = new St.Label({
            text: agentPhaseBadgeText(agent),
            style_class: 'ams-phase',
            y_align: Clutter.ActorAlign.CENTER,
        });
        badge.set_style(`color: ${agentPhaseBadgeColor(agent)};`);
        if (agent.run_active && String(agent.phase || '').trim().toUpperCase() === 'RUNNING') {
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
        if (agent.project)
            origin.push(`${agent.project}`);
        if (agent.branch)
            origin.push(`(${agent.branch})`);
        if (origin.length > 0) {
            row.add_child(new St.Label({
                text: origin.join(' '),
                style_class: 'ams-agent-origin',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        item.add_child(row);
        return item;
    }

    /** Tear down timers and the file monitor before the actor goes away. */
    destroy() {
        if (this._refreshId) {
            GLib.source_remove(this._refreshId);
            this._refreshId = 0;
        }
        if (this._pollId) {
            GLib.source_remove(this._pollId);
            this._pollId = 0;
        }
        if (this._flashId) {
            GLib.source_remove(this._flashId);
            this._flashId = 0;
        }
        if (this._runClockId) {
            GLib.source_remove(this._runClockId);
            this._runClockId = 0;
        }
        this._flashActors = [];
        this._runClockActors = [];
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

export default class AccModelStatusExtension extends Extension {
    /** Add the indicator to the panel. */
    enable() {
        this._button = new AccModelStatusButton(this);
        Main.panel.addToStatusArea('acc-model-status', this._button, 0, 'right');
    }

    /** Remove the indicator and every resource it owns. */
    disable() {
        this._button?.destroy();
        this._button = null;
    }
}
