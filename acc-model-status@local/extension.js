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
 *     so only agent terminals open on the active workspace are rendered. It
 *     never scans tmux, spawns a subprocess, or talks to the network; it watches
 *     the file and Shell window/workspace signals directly.
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
/** Agents listed per provider in the menu before collapsing into a summary. */
const MAX_MENU_AGENTS = 8;
/** Fallback icon for providers this extension ships no artwork for. */
const FALLBACK_ICON = 'acc-provider-generic-symbolic';
/** Phases contributing to the scoped provider counters. */
const RUNNING_PHASES = new Set(['RUNNING']);
const ATTENTION_PHASES = new Set(['WAITING', 'INTERRUPTED']);
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

/**
 * Filter the all-workspace ACC payload against titles of open windows here.
 *
 * Matching is one-to-one: a single terminal window can select at most one
 * payload row. This matters when historical sessions share the same summary;
 * one open window must never resurrect every matching detached session.
 * Provider and footer counts are recomputed from the selected rows.
 *
 * @param {object} payload - normalized document from {@link readPayload}.
 * @param {string[]} windowTitles - titles on the active GNOME workspace.
 * @returns {object} payload containing only agents backed by those windows.
 */
function scopePayloadToWindowTitles(payload, windowTitles) {
    const candidates = [];
    for (const provider of payload.providers) {
        const agents = Array.isArray(provider.agents) ? provider.agents : [];
        for (const agent of agents)
            candidates.push({provider, agent});
    }

    const selected = new Set();
    for (const title of windowTitles) {
        let best = null;
        let bestScore = 0;
        for (const candidate of candidates) {
            if (selected.has(candidate.agent))
                continue;
            const score = agentWindowMatchScore(candidate.agent, title);
            if (score > bestScore) {
                best = candidate;
                bestScore = score;
            }
        }
        if (best)
            selected.add(best.agent);
    }

    const providers = [];
    let totalLive = 0;
    let totalRunning = 0;
    for (const provider of payload.providers) {
        const sourceAgents = Array.isArray(provider.agents) ? provider.agents : [];
        const agents = sourceAgents.filter(agent => selected.has(agent));
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
     * Track identity/workspace changes for one window until it is unmanaged.
     *
     * @param {Meta.Window} win - live Shell window.
     */
    _trackWindow(win) {
        if (!win || this._windowSignals.has(win))
            return;
        const ids = [
            win.connect('workspace-changed', () => this._queueRefresh()),
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

    /** Return titles of every open window located on the active workspace. */
    _activeWorkspaceWindowTitles() {
        const activeWorkspace = global.workspace_manager.get_active_workspace();
        const windowTitles = [];
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window;
            if (!win || !win.located_on_workspace(activeWorkspace))
                continue;
            const title = win.get_title();
            if (title)
                windowTitles.push(title);
        }
        return windowTitles;
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
        const windowTitles = this._activeWorkspaceWindowTitles();
        const payload = published
            ? scopePayloadToWindowTitles(published, windowTitles)
            : null;
        this._box.destroy_all_children();
        this.menu.removeAll();

        if (!payload || payload.providers.length === 0) {
            this._renderEmpty(payload);
            return;
        }

        const stale = ageSeconds(payload.updatedAt) > STALE_SECONDS;
        for (const provider of payload.providers)
            this._renderProvider(provider, stale);
        this._renderMenu(payload, stale);
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
            payload ? 'No agent terminals on this workspace' : 'Agent Command Center not publishing',
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
     * Build the drop-down: a heading per provider plus its agents.
     *
     * @param {object} payload - normalized payload.
     * @param {boolean} stale - whether the payload is stale.
     */
    _renderMenu(payload, stale) {
        for (const provider of payload.providers) {
            const label = typeof provider.label === 'string' ? provider.label : 'Unknown';
            const running = Number(provider.running) || 0;
            const live = Number(provider.live) || 0;
            const heading = new PopupMenu.PopupMenuItem(
                `${label} — ${running} running / ${live} live`, {reactive: false});
            heading.label.add_style_class_name('ams-menu-heading');
            this.menu.addMenuItem(heading);

            const agents = Array.isArray(provider.agents) ? provider.agents : [];
            for (const agent of agents.slice(0, MAX_MENU_AGENTS))
                this.menu.addMenuItem(this._agentMenuItem(agent));
            if (agents.length > MAX_MENU_AGENTS) {
                this.menu.addMenuItem(new PopupMenu.PopupMenuItem(
                    `    … ${agents.length - MAX_MENU_AGENTS} more`, {reactive: false}));
            }
            this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        }

        const footer = stale
            ? `Stale — last update ${payload.updatedAt || 'unknown'}`
            : `${payload.totalRunning} running / ${payload.totalLive} live`;
        this.menu.addMenuItem(new PopupMenu.PopupMenuItem(footer, {reactive: false}));
    }

    /**
     * Build one agent row: workspace tag, colored phase badge, title, origin.
     *
     * The row mirrors the notes menu: ``W2`` for the GNOME workspace the
     * agent's project lives on (omitted when unknown), then the phase in the
     * shared notes palette (DONE green, RUNNING yellow, STOPPED red …).
     *
     * @param {object} agent - agent entry from the payload.
     * @returns {PopupMenu.PopupBaseMenuItem} the non-reactive row.
     */
    _agentMenuItem(agent) {
        const item = new PopupMenu.PopupBaseMenuItem({
            reactive: false,
            can_focus: false,
        });
        const row = new St.BoxLayout({style_class: 'ams-agent-row'});

        const workspace = Number(agent.workspace) || 0;
        if (workspace > 0) {
            row.add_child(new St.Label({
                text: `W${workspace}`,
                style_class: 'ams-workspace',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }

        const phase = String(agent.phase || 'UNKNOWN').trim().toUpperCase();
        const badge = new St.Label({
            text: phase,
            style_class: 'ams-phase',
            y_align: Clutter.ActorAlign.CENTER,
        });
        badge.set_style(`color: ${phaseColor(phase)};`);
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
