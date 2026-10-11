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
        this.strokes = [];
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
        stroke: () => { this.stroked = true; this.strokes.push({rect: this.draws.at(-1)?.rect, color: this.color, width: this.lineWidth, start: this.start, end: this.end}); },
    });
}

const St = {BoxLayout: Actor, DrawingArea: Actor, Label: Actor};
const Clutter = {ActorAlign: {CENTER: 0}};
const snippets = ['taskProgressValue', 'panelTaskProgressValue', 'panelProgressColor', 'panelProgressStateColor', 'panelProgressWave', 'panelProgressUnderway', 'panelProgressGoalActive', 'panelProgressOpacity', 'paintPanelTaskProgress', 'buildPanelTaskProgress',
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
const makeOwners = new Function('St', 'Clutter', 'GLib', 'services', 'print', `${source.slice(source.indexOf('const PHASE_COLORS ='), source.indexOf('};', source.indexOf('const PHASE_COLORS =')) + 2)}
${snippets.join('\n')}
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
check(bars.every(bar => bar.width === 52 && bar.height === 10), '52x10 drawing room preserves the 50x8 fill and outside outline');
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
check(visibleBars(button._panelProgress).every(bar => bar.width === 52 && !bar.text && !bar.reactive),
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

// Ready bars stay fully opaque and lighten from green to white on the running clock.
const ready = {session_id: 'ready', phase: 'DONE', unread_completion: true,
    ready_completed_at: '2026-10-10T10:00:00Z', workflow: {available: true, fraction: 1}};
const readyPayload = {providers: [{kind: 'codex', agents: [a, ready, b]}]};
button._updatePanelProgress(readyPayload, false);
let readyBar = button._panelProgressBars.get('ready');
const readyPaints = readyBar.repaints;
const runningPaints = button._panelProgressBars.get('a').repaints;
button._applyPanelProgressPulse(1500);
check(readyBar.draws.at(-1).color.slice(0, 3).every(channel => channel === 1),
    'unopened ready instance is painted white at the bright peak');
for (const [now, expectedColor, expectedRunning] of [
    [0, [34, 197, 94], 255], [750, [145, 226, 175], 166], [1500, [255, 255, 255], 77],
    [2250, [145, 226, 175], 166], [3000, [34, 197, 94], 255],
]) {
    button._applyPanelProgressPulse(now);
    const painted = readyBar.draws.at(-1);
    const rgb = painted.color.slice(0, 3).map(channel => Math.round(channel * 255));
    check(rgb.every((channel, index) => Math.abs(channel - expectedColor[index]) <= 1),
        'ready fill follows green → white → green in three seconds');
    check(readyBar.opacity === 255 && painted.color[3] === 1,
        'ready bars never fade darker through actor or fill opacity');
    check(painted.rect[2] === 50, 'white pulse retains full completed width');
    check(button._panelProgressBars.get('a').opacity === expectedRunning, 'running bar retains its dim cycle');
    check(button._panelProgressBars.get('b').opacity === 255, 'stopped bar remains steady');
}
check(readyBar.repaints > readyPaints, 'ready pulse repaints actual brighter pixels');
check(button._panelProgressBars.get('a').repaints === runningPaints, 'running pulse still uses compositor opacity only');
button._updatePanelProgress({providers: [{agents: [ready]}]}, false);
readyBar = button._panelProgressBars.get('ready');
check(timers.size === 1, 'ready-only panel keeps the single pulse timer');
button._updatePanelProgress({providers: [{agents: [ready]}]}, true);
check(timers.size === 0 && readyBar.opacity === 128, 'stale ready bar stops animation and stays dim');
check(Math.round(readyBar.draws.at(-1).color[0] * 255) === 34, 'stale completion stays green without a white pulse');
ready.unread_completion = false;
button._updatePanelProgress({providers: [{agents: [ready]}]}, false);
button._applyPanelProgressPulse(1500);
check(timers.size === 0 && readyBar.opacity === 255, 'acknowledged completion is steady');
check(Math.round(readyBar.draws.at(-1).color[0] * 255) === 34, 'acknowledgement restores the steady green fill');
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

// The focused terminal has a one-pixel teal edge, independent of its workspace.
const focused = {...a, session_id: 'focus', focused: true, workspace: 1,
    workflow: {available: true, fraction: 0.8}};
const nearby = {...b, session_id: 'nearby', focused: false, workspace: 1, current_workspace: true};
const focusPayload = {providers: [{kind: 'codex', agents: [focused, nearby]}]};
button._updatePanelProgress(focusPayload, false);
const focusedBar = button._panelProgressBars.get('focus');
const nearbyBar = button._panelProgressBars.get('nearby');
check(focusedBar.strokes.length === 1, 'selected instance paints a border');
const edge = focusedBar.strokes.at(-1);
check(edge.width === 1 && JSON.stringify(edge.rect) === '[0.5,0.5,51,9]',
    'one-pixel edge sits outside the 50 by 8 fill in reserved drawing room');
check(JSON.stringify(edge.color.slice(0,3).map(channel => Math.round(channel*255))) === '[0,255,230]' &&
    edge.color[3] === 0.8, 'subtle teal matches CLI selection color #00ffe6');
check(nearbyBar.strokes.length === 0, 'same workspace does not imply selection');
check(JSON.stringify(focusedBar.draws.at(-2).rect) === '[1,1,40,8]',
    'outward border preserves the measured fill dimensions and origin');
check(JSON.stringify(focusedBar.draws.at(-3).rect) === '[1,1,50,8]',
    'full progress track remains 50x8 inside reserved drawing room');
const track = focusedBar.draws.at(-3).rect;
check(edge.rect[0] + edge.width / 2 <= track[0] &&
    edge.rect[1] + edge.width / 2 <= track[1] &&
    edge.rect[0] + edge.rect[2] - edge.width / 2 >= track[0] + track[2] &&
    edge.rect[1] + edge.rect[3] - edge.width / 2 >= track[1] + track[3],
    'teal stroke never overlaps any progress fill pixel');
const oldEdges = focusedBar.strokes.length;
focused.focused = false;
nearby.focused = true;
button._updatePanelProgress(focusPayload, false);
check(button._panelProgressBars.get('focus') === focusedBar && button._panelProgressBars.get('nearby') === nearbyBar,
    'focus transfer retains bar actor identity');
check(focusedBar.strokes.length === oldEdges && nearbyBar.strokes.length === 1,
    'outline follows focus and clears from the old selection');
nearby.focused = false;
button._updatePanelProgress(focusPayload, false);
check(nearbyBar.strokes.length === 1, 'focusing outside agent terminals clears selection');
for (const agent of [
    {...a, focused: true}, {...b, focused: true}, {...ready, focused: true},
    {phase: 'STOPPED', focused: true, workflow: {available: false}},
]) {
    const bar = owners.buildPanelTaskProgress(agent);
    bar._amsPulseAt = 1500;
    bar.queue_repaint();
    check(bar.strokes.length === 1 && bar.width === 52 && bar.height === 10,
        'focused running, stopped, ready and unavailable bars all retain a compact outline');
    if (agent.unread_completion)
        check(bar.draws.at(-2).color.slice(0,3).every(channel => channel === 1),
            'teal outline does not replace the ready white fill');
}
nearby.focused = true;
button._updatePanelProgress(focusPayload, true);
check(nearbyBar.strokes.length === 2 && nearbyBar.opacity === 128, 'stale selected bar keeps the outline and stale opacity');
button._updatePanelProgress(null, false);
check(timers.size === 0, 'selection does not start an extra animation source');
print('PASS: selected instance uses a one-pixel teal edge matching CLI focus and follows focus changes.');

// State colors must preserve the measured fill and every existing alpha.
for (const [state, expected] of [
    [{phase: 'RUNNING', run_active: true, goal_active: true}, [137, 133, 81]],
    [{phase: 'RATE', goal_active: true}, [244, 114, 182]],
    [{phase: 'RATE'}, [244, 114, 182]],
    [{phase: 'RUNNING', run_active: true, goal_active: false}, [230, 194, 0]],
    [{phase: 'RUNNING', run_active: true}, [230, 194, 0]],
    [{phase: 'RUNNING', run_active: false, goal_active: false}, [137, 133, 81]],
    [{phase: 'STOPPED', goal_active: true}, [137, 133, 81]],
    [{phase: 'DONE', goal_active: true, ready_completed_at: '2026-10-10T10:00:00Z'}, [34, 197, 94]],
]) {
    const agent = {...state, workflow: {available: true, fraction: .5}};
    const bar = owners.buildPanelTaskProgress(agent);
    bar._amsPulseAt = 0;
    bar.queue_repaint();
    const fill = bar.draws.at(-1);
    check(JSON.stringify(fill.color.slice(0, 3).map(channel => Math.round(channel * 255))) === JSON.stringify(expected),
        `state color follows ${JSON.stringify(state)}`);
    check(bar.draws[0].color[3] === .22 && fill.color[3] === 1,
        'state colors preserve track and fill alpha');
    check(fill.rect[2] === (state.phase === 'DONE' ? 50 : 25), 'state color preserves measured width');
}
for (const state of [{phase: 'RATE'}, {phase: 'RUNNING', run_active: true, goal_active: true}]) {
    const bar = owners.buildPanelTaskProgress({...state, workflow: {available: false}});
    bar.queue_repaint();
    check(bar.draws.length === 1 && bar.draws[0].color[3] === 1,
        'unavailable state preserves neutral track and alpha');
    check(JSON.stringify(bar.draws[0].color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[48,52,64]',
        'unavailable workflow stays neutral');
}
print('PASS: goal and RATE fills match state colors without changing progress or opacity.');

check(/\.ams-panel-workspace\s*\{[^}]*spacing:\s*6px;/.test(css),
    'same-workspace bars have a slightly wider six-pixel actor gap');
print('PASS: outward focus outline leaves the 50x8 fill clear and bars have wider spacing.');

// Active ordinary progress uses timer yellow at every measured endpoint.
for (const fraction of [0, .25, .5, .75, 1]) {
    const bar = owners.buildPanelTaskProgress({phase: 'RUNNING', run_active: true,
        goal_active: false, workflow: {available: true, fraction}});
    bar.queue_repaint();
    const painted = bar.draws.at(-1);
    check(JSON.stringify(painted.color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[230,194,0]',
        'ordinary active progress matches RUNNING yellow at every fraction');
    check(painted.color[3] === (fraction > 0 ? 1 : .22), 'ordinary yellow preserves alpha');
    check(painted.rect[2] === (fraction > 0 ? fraction * 50 : 50), 'ordinary yellow preserves track/fill width');
}
print('PASS: ordinary active bars match timer yellow without changing measured widths or opacity.');

// Goal identity is a one-pixel bottom edge, never a replacement for progress.
for (const focused of [false, true]) {
    for (const [fraction, expected] of [[0, [239,68,68]], [.5, [137,133,81]], [1, [34,197,94]]]) {
        const agent = {phase: 'RUNNING', run_active: true, goal_active: true, focused,
            workflow: {available: true, fraction}};
        const bar = owners.buildPanelTaskProgress(agent);
        bar.queue_repaint();
        check(JSON.stringify(bar.draws[0].color.slice(0,3).map(value => Math.round(value*255))) === JSON.stringify(expected),
            'goal retains the measured progress gradient');
        const goalEdge = bar.strokes.at(-1);
        check(bar.strokes.length === (focused ? 2 : 1) && goalEdge.width === 1,
            'goal has one bottom stroke as thick as the teal focus stroke');
        check(JSON.stringify(goalEdge.start) === '[1,9.5]' && JSON.stringify(goalEdge.end) === '[51,9.5]',
            'goal stroke occupies bottom outer room only and never touches the fill');
        check(JSON.stringify(goalEdge.color) === JSON.stringify([168/255,85/255,247/255,.8]),
            'goal bottom uses goal purple and existing outline alpha');
        check(bar.draws[0].color[3] === .22 && bar.width === 52 && bar.height === 10,
            'goal outline preserves alpha and actor dimensions');
        if (focused)
            check(JSON.stringify(bar.strokes[0].color) === JSON.stringify([0,1,230/255,.8]),
                'focused goal retains teal outline underneath purple bottom');
    }
}
for (const state of [
    {phase: 'RUNNING', run_active: true, goal_active: false},
    {phase: 'RUNNING', run_active: false, goal_active: true},
    {phase: 'DONE', goal_active: true, unread_completion: true},
    {phase: 'RATE', goal_active: true},
]) {
    const bar = owners.buildPanelTaskProgress({...state, workflow: {fraction: .5}});
    bar.queue_repaint();
    check(bar.strokes.length === 0, 'ordinary, inactive, ready and RATE bars have no active-goal edge');
}
print('PASS: goal bars keep the gradient and only a one-pixel purple bottom edge; focus and opacity remain intact.');
