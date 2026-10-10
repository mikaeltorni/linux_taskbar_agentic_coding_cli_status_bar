// Run the real taskbar rendering owners under gjs with isolated actor stubs.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes);

/** Extract a JavaScript owner for isolated execution.
 * Parameters: name - function or method name; method - whether it is a class method.
 * Returns: the complete declaration text.
 */
function declaration(name, method = false) {
    print(`name=${name} method=${method}`);
    const marker = method ? `\n    ${name}(` : `\nfunction ${name}(`;
    const start = source.indexOf(marker);
    if (start < 0)
        throw Error(`Missing public rendering owner ${name}`);
    let depth = 0;
    for (let i = source.indexOf('{', start); i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}' && --depth === 0) {
            const result = source.slice(start + 1, i + 1);
            print(result);
            return result;
        }
    }
    throw Error(`Unbalanced declaration ${name}`);
}

class Actor {
    /** Store actor properties without connecting to a desktop.
     * Parameters: this - mock actor; options - construction properties.
     * Returns: undefined.
     */
    constructor(options = {}) {
        print(`this=${this} options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        this.children = [];
        this.draws = [];
        this.repaints = 0;
        print(undefined);
    }
    add_child = child => { child._parent = this; this.children.push(child); };
    remove_child = child => { child._parent = null; this.children = this.children.filter(item => item !== child); };
    get_parent = () => this._parent || null;
    destroy_all_children = () => { this.children.forEach(child => child.destroy()); this.children = []; };
    /** Destroy the isolated mock actor.
     * Parameters: this - mock actor receiver.
     * Returns: undefined.
     */
    destroy() {
        print(`this=${this}`);
        this.destroyed = true;
        print(undefined);
    }
    connect = (signal, callback) => { this[signal] = callback; return 1; };
    queue_repaint = () => { this.repaints++; this.repaint?.(this); };
    get_surface_size = () => [this.width, this.height];
    get_context = () => ({
        setSourceRGBA: (...rgb) => { this.color = rgb; },
        rectangle: (...rect) => this.draws.push({rect, color: this.color}),
        fill: () => {}, $dispose: () => {},
        setLineWidth: width => { this.lineWidth = width; },
        setDash: (dash, offset) => { this.dash = dash; },
        moveTo: (...point) => { this.start = point; },
        lineTo: (...point) => { this.end = point; },
        stroke: () => { this.stroked = true; },
    });
}

const St = {BoxLayout: Actor, DrawingArea: Actor, Label: Actor};
const Clutter = {ActorAlign: {CENTER: 0}};
const snippets = ['taskProgressValue', 'panelTaskProgressValue', 'panelProgressColor', 'panelProgressUnderway', 'panelProgressOpacity', 'paintPanelTaskProgress', 'buildPanelTaskProgress',
    'workspaceSortValue', 'windowDisplaySortValue', 'monitorSortValue', 'compareWorkspaceEntries', 'groupWorkspaceEntries',
    'paintPanelWorkspaceSeparator', 'buildPanelWorkspaceSeparator']
    .map(name => declaration(name));
const timers = new Map();
let nextTimer = 1;
const GLib = {PRIORITY_DEFAULT: 0, SOURCE_CONTINUE: true,
    timeout_add: (_priority, _delay, callback) => { const id = nextTimer++; timers.set(id, callback); return id; },
    source_remove: id => timers.delete(id),
};
let snapshot = null;
const services = {readPayload: () => snapshot, scopePayloadToWindows: value => value,
    withWindowDisplayIds: value => value, readWindowDisplayIds: () => new Map(),
    activeTaskEstimates: () => [], longestTaskEstimate: () => ({}), formatPanelTaskEstimate: () => '',
    ageSeconds: () => 0, STALE_SECONDS: 180, unreadCompletionCount: () => 0,
    saveUnreadStates: () => {}, log: () => {}, workspaceAttentionCounts: () => [{workspace: 5, count: 1, current: false, label: 'W5:1'}], panelHarnessRateRows: () => [],
};
const makeOwners = new Function('St', 'Clutter', 'GLib', 'services', 'print', `${snippets.join('\n')}
    const {readPayload, scopePayloadToWindows, withWindowDisplayIds, readWindowDisplayIds,
        activeTaskEstimates, longestTaskEstimate, formatPanelTaskEstimate, ageSeconds, STALE_SECONDS,
        unreadCompletionCount, saveUnreadStates, log, workspaceAttentionCounts, panelHarnessRateRows} = services;
    class Destroyer extends St.BoxLayout { ${declaration('destroy', true)} }
    return {panelTaskProgressValue, panelProgressUnderway, panelProgressOpacity, paintPanelTaskProgress, buildPanelTaskProgress,
        Destroyer,
        updater: {${['_refresh', '_updatePanelProgress', '_syncPanelPulseTimer', '_applyPanelProgressPulse',
            ...(source.includes('\n    _renderWorkspaceAttention(') ? ['_renderWorkspaceAttention'] : [])].map(name => declaration(name, true)).join(',')}}};`);
const owners = makeOwners(St, Clutter, GLib, services, print);
const check = (condition, message) => { if (!condition) throw Error(message); };
const visibleBars = container => container.children.flatMap(child =>
    child.style_class === 'ams-panel-workspace' ? child.children : []);
const a = {session_id: 'a', phase: 'RUNNING', run_active: true, workflow: {available: true, fraction: 0.25}};
const b = {session_id: 'b', phase: 'STOPPED', workflow: {available: true, fraction: 0.6}};
const payload = {providers: [{kind: 'codex', agents: [a]}, {kind: 'claude', agents: [b]}]};
const button = Object.assign({_box: new Actor()}, owners.updater);
button._updatePanelProgress(payload, false);
let bars = visibleBars(button._panelProgress);
check(bars.length === 2, 'one bar for every open agent');
check(bars.every(bar => bar.width === 50 && bar.height === 8), '50px compact bars');
check(bars[0].draws.at(-1).rect[2] === 12.5, 'actual 25% fill');
check(bars[1].draws.at(-1).rect[2] === 30, 'actual 60% fill');
const retained = bars[0];
a.workflow.fraction = 0.8;
button._updatePanelProgress(payload, false);
bars = visibleBars(button._panelProgress);
check(bars[0] === retained, 'reuse actor during progress updates');
check(bars[0].draws.at(-1).rect[2] === 40, 'progress refresh to 80%');
const cases = [
    [{phase: 'DONE', ready_completed_at: '2026-10-10T10:00:00Z'}, 1],
    [{phase: 'DONE', unread_completion: true}, 1],
    [{workflow: {available: true, fraction: 0}}, 0],
    [{workflow: {available: true, fraction: 1}}, 1],
    [{workflow: {available: false, fraction: 0}}, null],
    [{workflow: {fraction: NaN}}, null],
    [{workflow: {fraction: Infinity}}, null],
    [{workflow: {fraction: '0.5'}}, null],
    [{workflow: {fraction: -0.2}}, 0],
    [{workflow: {fraction: 1.2}}, 1],
    [{}, null],
];
for (const [agent, expected] of cases)
    check(owners.panelTaskProgressValue(agent) === expected, 'progress endpoints and unavailable data');
button._updatePanelProgress({providers: [{agents: [b]}]}, true);
check(button._panelProgress.children.length === 1, 'remove closed agents');
button._updatePanelProgress(null, false);
check(button._panelProgress.children.length === 0, 'clear unavailable snapshot');
print('PASS: per-agent taskbar bars use real progress and update without replacing actors.');

for (const [fraction, expected] of [
    [0, [239, 68, 68]], [0.25, [188, 100, 75]], [0.5, [137, 133, 81]],
    [0.75, [85, 165, 88]], [1, [34, 197, 94]],
]) {
    const bar = owners.buildPanelTaskProgress({workflow: {fraction}});
    bar.queue_repaint();
    const rgb = bar.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255));
    check(JSON.stringify(rgb) === JSON.stringify(expected), 'red-to-green gradient at actual progress');
    if (fraction > 0)
        check(bar.draws.at(-1).rect[2] === fraction * 50, 'color never changes measured fill width');
    else
        check(bar.draws.length === 1 && bar.draws[0].color[3] < 1, '0% shows a faint red track with no invented fill');
}
print('PASS: progress color moves continuously from red at 0% to green at 100%.');

for (const [now, opacity] of [[0, 255], [750, 166], [1500, 77], [2250, 166], [3000, 255], [4500, 77]])
    check(owners.panelProgressOpacity(true, now, false) === opacity, 'full → 30% → full in three seconds');
for (const phase of ['DONE', 'STOPPED', 'ATTN', 'INTERRUPTED', 'TRANSFERRED', 'DRAFT'])
    check(owners.panelProgressUnderway({...a, phase}) === false, 'only underway agents pulse');
check(owners.panelProgressUnderway(a) === true, 'active running turn is underway');
check(owners.panelProgressUnderway({...a, run_active: false}) === false, 'inactive turn stays steady');
check(owners.panelProgressOpacity(false, 1500, false) === 255, 'idle snapshot stays steady');
check(owners.panelProgressOpacity(true, 1500, true) === 128, 'stale snapshot stays steadily dim');
button._updatePanelProgress(payload, false);
check(timers.size === 1, 'one pulse timer for all underway bars');
button._updatePanelProgress(payload, false);
check(timers.size === 1, 'updates retain the existing animation source');
const runningBar = button._panelProgressBars.get('a');
const stoppedBar = button._panelProgressBars.get('b');
const paints = runningBar.repaints;
button._applyPanelProgressPulse(1500);
check(runningBar.opacity === 77 && stoppedBar.opacity === 255, 'live tick dims only underway bar');
check(runningBar.repaints === paints, 'pulse updates compositor opacity without repainting progress pixels');
button._updatePanelProgress(payload, true);
check(timers.size === 0, 'stale data stops animation');
button._updatePanelProgress({providers: [{agents: [b]}]}, false);
check(timers.size === 0, 'idle agents have no animation source');
button._updatePanelProgress(payload, false);
check(timers.size === 1, 'new active work starts animation again');
button._updatePanelProgress(null, false);
check(timers.size === 0, 'closing all agents removes animation source');
button._updatePanelProgress(payload, false);
const destroyer = new owners.Destroyer();
Object.assign(destroyer, {_panelPulseId: button._panelPulseId, _taskRows: new Map(),
    _shellSignals: [], _windowSignals: new Map()});
destroyer.destroy();
check(timers.size === 0 && destroyer.destroyed === true, 'disable releases timer and destroys the actor');
print('PASS: underway bars pulse smoothly and animation sources stop on idle, stale data and disable.');

const tasks = [
    {session_id: 'ws2-10', workspace: 2, window_display_id: 10, monitor: 0, workflow: {fraction: .2}},
    {session_id: 'ws1-2', workspace: 1, window_display_id: 2, monitor: 2, workflow: {fraction: .4}},
    {session_id: 'ws1-1', workspace: 1, window_display_id: 1, monitor: 1, workflow: {fraction: .6}},
    {session_id: 'ws2-2', workspace: 2, window_display_id: 2, monitor: 1, workflow: {fraction: .8}},
    {session_id: 'unknown', workspace: 0, workflow: {fraction: .1}},
];
const grouped = {providers: [{kind: 'claude', agents: [tasks[0], tasks[1], tasks[4]]},
    {kind: 'codex', agents: [tasks[2], tasks[3]]}]};
button._updatePanelProgress(grouped, false);
const children = button._panelProgress.children;
check(JSON.stringify(children.map(child => child.style_class)) === JSON.stringify([
    'ams-panel-workspace', 'ams-panel-workspace-divider', 'ams-panel-workspace',
    'ams-panel-workspace-divider', 'ams-panel-workspace']), 'vertical dividers only between workspace groups');
const groups = children.filter(child => child.style_class === 'ams-panel-workspace');
check(JSON.stringify(groups.map(group => group._amsWorkspace)) === '[1,2,0]', 'numeric workspace order with unknown last');
check(JSON.stringify(visibleBars(button._panelProgress).map(bar => bar._amsAgent.session_id)) ===
    '["ws1-1","ws1-2","ws2-2","ws2-10","unknown"]', 'agents use workspace and numeric window order across providers');
check(visibleBars(button._panelProgress).every(bar => bar.width === 50 && !bar.text && !bar.reactive),
    'bars add no labels, percentages as text, or extra click targets');
const dividers = children.filter(child => child.style_class === 'ams-panel-workspace-divider');
for (const divider of dividers) {
    divider.queue_repaint();
    check(JSON.stringify(divider.dash) === '[1,3]', 'dotted divider');
    check(divider.start[0] === divider.end[0] && divider.end[1] === divider.height && divider.height === 16,
        'divider follows the vertical axis across its height');
}
const before = groups[0];
button._updatePanelProgress(grouped, false);
check(button._panelProgress.children[0] === before, 'stable updates retain workspace groups');
tasks[1].workspace = 2;
button._updatePanelProgress(grouped, false);
check(button._panelProgress.children[0].children.length === 1, 'moved agent leaves its old workspace group');
button._updatePanelProgress({providers: [{kind: 'codex', agents: [tasks[2], {...tasks[3], workspace: 1}]}]}, false);
check(button._panelProgress.children.length === 1 && visibleBars(button._panelProgress).length === 2,
    'one workspace has no divider even across monitors');
print('PASS: compact bars group by workspace with vertical dotted dividers and no added details.');

Object.assign(button, {_file: {get_parent: () => ({get_child: () => ({})})},
    _unreadStates: new Map(), _unreadCount: 0, _taskRows: new Map(),
    menu: {isOpen: false, removeAll: () => {}},
    _openWindowDescriptors: () => [], _syncMuteIndicator: () => {}, _renderHarnessRates: () => {},
    _renderProvider: () => {}, _renderPanelHarnessRates: () => {},
    _renderMenu: () => {}, _updatePanelTaskEstimate: () => {}, _syncGlowTimer: () => {}, _syncRunClockTimer: () => {},
});
snapshot = payload;
button._refresh();
const retainedStrip = button._panelProgress;
const retainedBar = visibleBars(retainedStrip)[0];
for (let i = 0; i < 5; i++) {
    a.workflow.fraction = 0.5 + i * 0.1;
    button._refresh();
    check(button._panelProgress === retainedStrip && visibleBars(retainedStrip)[0] === retainedBar && !retainedBar.destroyed,
        'closed-menu snapshot refreshes retain painted bar actors');
}
check(retainedStrip.get_parent() === button._box, 'retained strip remains attached to panel');
print('PASS: real closed-menu snapshot refreshes preserve existing progress actors and painted surfaces.');

// Ready bars rise toward full opacity on the same clock as running bars dim.
const ready = {session_id: 'ready', phase: 'DONE', unread_completion: true,
    ready_completed_at: '2026-10-10T10:00:00Z', workflow: {available: true, fraction: 1}};
const readyPayload = {providers: [{kind: 'codex', agents: [a, ready, b]}]};
button._updatePanelProgress(readyPayload, false);
let readyBar = button._panelProgressBars.get('ready');
const readyPaints = readyBar.repaints;
for (const [now, expectedReady, expectedRunning] of [
    [0, 166, 255], [750, 210, 166], [1500, 255, 77],
    [2250, 210, 166], [3000, 166, 255],
]) {
    button._applyPanelProgressPulse(now);
    check(readyBar.opacity === expectedReady, 'unopened ready bar brightens then returns in three seconds');
    check(button._panelProgressBars.get('a').opacity === expectedRunning, 'running bar retains its dim cycle');
    check(button._panelProgressBars.get('b').opacity === 255, 'stopped bar remains steady');
}
check(readyBar.repaints === readyPaints, 'ready pulse changes compositor opacity without repainting');
button._updatePanelProgress({providers: [{agents: [ready]}]}, false);
readyBar = button._panelProgressBars.get('ready');
check(timers.size === 1, 'ready-only panel keeps the single pulse timer');
button._updatePanelProgress({providers: [{agents: [ready]}]}, true);
check(timers.size === 0 && readyBar.opacity === 128, 'stale ready bar stops animation and stays dim');
ready.unread_completion = false;
button._updatePanelProgress({providers: [{agents: [ready]}]}, false);
button._applyPanelProgressPulse(1500);
check(timers.size === 0 && readyBar.opacity === 255, 'acknowledged completion is steady');
ready.unread_completion = true;
button._updatePanelProgress({providers: [{agents: [ready]}]}, false);
const readyDestroyer = new owners.Destroyer();
Object.assign(readyDestroyer, {_panelPulseId: button._panelPulseId, _taskRows: new Map(),
    _shellSignals: [], _windowSignals: new Map()});
readyDestroyer.destroy();
check(timers.size === 0, 'disable releases ready-only animation source');
button._panelPulseId = 0;
button._updatePanelProgress(null, false);
const [, cssBytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/stylesheet.css`).load_contents(null);
const css = new TextDecoder().decode(cssBytes);
check(!source.includes('ams-unread-glow') && !css.includes('.ams-unread-glow'), 'no broad green panel glow');
check(!source.includes('_syncGlowTimer'), 'no separate broad glow timer');
print('PASS: ready instances brighten without a broad glow; acknowledgement, stale data and disable stop animation.');

// A public snapshot refresh must leave ready workspace badges out of the panel.
ready.workspace = 5;
snapshot = {providers: [{kind: 'codex', agents: [ready]}]};
for (let refresh = 0; refresh < 2; refresh++) {
    button._refresh();
    check(!button._box.children.some(actor => /^W\d+:\d+$/.test(actor.text || '')),
        'ready workspace W5:1 adds no label to the panel');
    check(visibleBars(button._panelProgress).length === 1,
        'workspace badge removal retains the ready instance bar');
    button._applyPanelProgressPulse(1500);
    check(button._panelProgressBars.get('ready').opacity === 255,
        'ready bright pulse survives snapshot refresh without workspace badges');
}
button._updatePanelProgress(null, false);
check(timers.size === 0, 'empty snapshot cleans up the ready timer');
print('PASS: repeated ready snapshots render the instance bar without a W5:1 workspace badge.');
