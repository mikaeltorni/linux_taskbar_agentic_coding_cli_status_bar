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
    get_children = () => this.children;
    add_style_class_name = name => { this.styles = [...(this.styles || []), name]; };
    remove_style_class_name = name => { this.styles = (this.styles || []).filter(style => style !== name); };
    destroy_all_children = () => { this.children.forEach(child => child.destroy()); this.children = []; };
    /** Destroy the isolated mock actor.
     * Parameters: this - mock actor receiver.
     * Returns: undefined.
     */
    destroy() {
        print(`this=${this}`);
        this.destroy_all_children();
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
        setLineCap: cap => { this.lineCap = cap; },
        setOperator: operator => { this.operator = operator; },
        setDash: (dash, offset) => { this.dash = dash; },
        moveTo: (...point) => { this.start = point; },
        lineTo: (...point) => { this.end = point; },
        stroke: () => { this.stroked = true; this.strokes.push({rect: this.draws.at(-1)?.rect, color: this.color, width: this.lineWidth, operator: this.operator, start: this.start, end: this.end}); },
    });
}

const St = {BoxLayout: Actor, DrawingArea: Actor, Label: Actor, Widget: Actor};
const Clutter = {ActorAlign: {CENTER: 0, FILL: 1, END: 2, START: 3}, BinLayout: class {}};
const snippets = ['phaseColor', 'agentPhaseBadgeColor', 'taskProgressValue', 'panelTaskProgressValue', 'panelProgressColor', 'panelProgressStateColor', 'panelProgressWave', 'panelProgressUnderway', 'panelProgressGoalActive', 'panelProgressOpacity', 'panelProgressHostOpacity', 'paintPanelTaskProgress', 'buildPanelTaskProgress',
    'workspaceSortValue', 'windowDisplaySortValue', 'monitorSortValue', 'compareWorkspaceEntries', 'groupWorkspaceEntries',
    'paintPanelWorkspaceSeparator', 'buildPanelWorkspaceSeparator', 'taskEstimateText',
    ...(source.includes('function buildPanelTaskCell(') ? ['panelAgentEstimateText', 'buildPanelTaskCell'] : []),
    ...(source.includes('function panelAgentRuntimeText(') ? ['formatPanelRunDuration', 'panelAgentRuntimeText'] : []),
    ...(source.includes('function panelWorkspaceColor(') ? ['panelWorkspaceColor'] : []),
    ...(source.includes('function buildPanelMonitorSeparator(') ? ['paintPanelMonitorSeparator', 'buildPanelMonitorSeparator'] : [])]
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
    ageSeconds: () => 0, STALE_SECONDS: 180, unreadCompletionCount: () => 0,
    saveUnreadStates: () => {}, log: () => {}, workspaceAttentionCounts: () => [{workspace: 5, count: 1, current: false, label: 'W5:1'}], panelHarnessRateRows: () => [],
};
const makeOwners = new Function('St', 'Clutter', 'GLib', 'services', 'print', 'Cairo', `${source.slice(source.indexOf('const PHASE_COLORS ='), source.indexOf('};', source.indexOf('const PHASE_COLORS =')) + 2)}
${snippets.join('\n')}
    const {readPayload, scopePayloadToWindows, withWindowDisplayIds, readWindowDisplayIds,
        ageSeconds, STALE_SECONDS,
        unreadCompletionCount, saveUnreadStates, log, workspaceAttentionCounts, panelHarnessRateRows} = services;
    class Destroyer extends St.BoxLayout { ${declaration('destroy', true)} }
    return {agentPhaseBadgeColor, panelTaskProgressValue, panelProgressUnderway, panelProgressOpacity, paintPanelTaskProgress, buildPanelTaskProgress,
        Destroyer, paintPanelWorkspaceSeparator,
        updater: {${['_refresh', '_updatePanelProgress', '_syncPanelPulseTimer', '_applyPanelProgressPulse',
            ...(source.includes('\n    _updatePanelTaskEstimate(') ? ['_updatePanelTaskEstimate'] : []),
            ...(source.includes('\n    _renderWorkspaceAttention(') ? ['_renderWorkspaceAttention'] : [])].map(name => declaration(name, true)).join(',')}}};`);
const owners = makeOwners(St, Clutter, GLib, services, print, imports.cairo);
const check = (condition, message) => { if (!condition) throw Error(message); };
const visibleBars = container => container.children.flatMap(child =>
    child.style_class === 'ams-panel-workspace-group' ? child.children[1].children.filter(cell => cell.style_class === 'ams-panel-task-cell').map(cell => cell.children[0]) : []);
const a = {session_id: 'a', phase: 'RUNNING', run_active: true, workflow: {available: true, fraction: 0.25}};
const b = {session_id: 'b', phase: 'STOPPED', workflow: {available: true, fraction: 0.6}};
const payload = {providers: [{kind: 'codex', agents: [a]}, {kind: 'claude', agents: [b]}]};
const button = Object.assign({_box: new Actor()}, owners.updater);
button._updatePanelProgress(payload, false);
let bars = visibleBars(button._panelProgress);
check(bars.length === 2, 'one bar for every open agent');
check(bars.every(bar => bar.width === 52 && bar.height === 11), '52x11 drawing room preserves the 50x8 fill and outside outline');
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
    'ams-panel-workspace-group', 'ams-panel-workspace-divider', 'ams-panel-workspace-group',
    'ams-panel-workspace-divider', 'ams-panel-workspace-group']), 'vertical dividers only between workspace groups');
const groups = children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(JSON.stringify(groups.map(group => group._amsWorkspace)) === '[1,2,0]', 'numeric workspace order with unknown last');
check(JSON.stringify(visibleBars(button._panelProgress).map(bar => bar._amsAgent.session_id)) ===
    '["ws1-1","ws1-2","ws2-2","ws2-10","unknown"]', 'agents use workspace and numeric window order across providers');
check(visibleBars(button._panelProgress).every(bar => bar.width === 52 && !bar.text && !bar.reactive),
    'bars add no labels, percentages as text, or extra click targets');
const dividers = children.filter(child => child.style_class === 'ams-panel-workspace-divider');
for (const divider of dividers) {
    check(divider.height === undefined && divider.y_expand === true && divider.y_align === Clutter.ActorAlign.FILL,
        'workspace divider stretches through the full panel instead of stopping at sixteen pixels');
    divider.height = 48;
    divider.queue_repaint();
    check(JSON.stringify(divider.dash) === '[0.01,5.99]' && divider.lineWidth === 2 && divider.lineCap === imports.cairo.LineCap.ROUND,
        'larger round workspace dots stay visually distinct from solid monitor lines');
    check(divider.start[0] === divider.end[0] && divider.end[1] === divider.height && divider.height === 48,
        'divider follows the vertical axis across its height');
}
// Native Cairo rejects zero-length dash values even though actor mocks accept them.
const workspaceSurface = new imports.cairo.ImageSurface(imports.cairo.Format.ARGB32, 6, 48);
owners.paintPanelWorkspaceSeparator({get_surface_size: () => [6, 48],
    get_context: () => new imports.cairo.Context(workspaceSurface)});
print('PASS: workspace dots paint through the native Cairo binding.');
const before = groups[0];
button._updatePanelProgress(grouped, false);
check(button._panelProgress.children[0] === before, 'stable updates retain workspace groups');
tasks[1].workspace = 2;
button._updatePanelProgress(grouped, false);
check(button._panelProgress.children[0].children[1].children.length === 1, 'moved agent leaves its old workspace group');
button._updatePanelProgress({providers: [{kind: 'codex', agents: [tasks[2], {...tasks[3], workspace: 1}]}]}, false);
check(button._panelProgress.children.length === 1 && visibleBars(button._panelProgress).length === 2,
    'one workspace has no divider even across monitors');
print('PASS: compact bars group by workspace with vertical dotted dividers and no added details.');

Object.assign(button, {_file: {get_parent: () => ({get_child: () => ({})})},
    _unreadStates: new Map(), _unreadCount: 0, _taskRows: new Map(),
    menu: {isOpen: false, removeAll: () => {}},
    _openWindowDescriptors: () => [], _syncMuteIndicator: () => {}, _renderHarnessRates: () => {},
    _renderProvider: () => {}, _renderPanelHarnessRates: () => {},
    _renderMenu: () => {}, _renderEmpty: () => {}, _syncGlowTimer: () => {}, _syncRunClockTimer: () => {},
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
    check(bar.strokes.length === 1 && bar.width === 52 && bar.height === 11,
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
    [{phase: 'RUNNING', run_active: true, goal_active: false}, [137, 133, 81]],
    [{phase: 'RUNNING', run_active: true}, [137, 133, 81]],
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
    bar._amsPulseAt = 0;
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

// Active ordinary progress tracks completion from red to green.
for (const [fraction, expected] of [[0,[239,68,68]], [.25,[188,100,75]], [.5,[137,133,81]], [.75,[85,165,88]], [1,[34,197,94]]]) {
    const bar = owners.buildPanelTaskProgress({phase: 'RUNNING', run_active: true,
        goal_active: false, workflow: {available: true, fraction}});
    bar.queue_repaint();
    const painted = bar.draws.at(-1);
    check(JSON.stringify(painted.color.slice(0, 3).map(channel => Math.round(channel * 255))) === JSON.stringify(expected),
        'ordinary active progress transitions red to green at every fraction');
    check(painted.color[3] === (fraction > 0 ? 1 : .22), 'ordinary gradient preserves alpha');
    check(painted.rect[2] === (fraction > 0 ? fraction * 50 : 50), 'ordinary gradient preserves track/fill width');
}
print('PASS: ordinary active bars transition red to green without changing measured widths or opacity.');

// Goal identity is a three-pixel bottom edge, never a replacement for progress.
for (const focused of [false, true]) {
    for (const [fraction, expected] of [[0, [239,68,68]], [.5, [137,133,81]], [1, [34,197,94]]]) {
        const agent = {phase: 'RUNNING', run_active: true, goal_active: true, focused,
            workflow: {available: true, fraction}};
        const bar = owners.buildPanelTaskProgress(agent);
        bar._amsPulseAt = 0;
        bar.queue_repaint();
        check(JSON.stringify(bar.draws[0].color.slice(0,3).map(value => Math.round(value*255))) === JSON.stringify(expected),
            'goal retains the measured progress gradient');
        const goalEdge = bar.strokes.at(-1);
        check(bar.strokes.length === (focused ? 2 : 1) && goalEdge.width === 3,
            'goal has one bottom stroke three times as thick as the teal focus stroke');
        check(JSON.stringify(goalEdge.start) === '[1,9.5]' && JSON.stringify(goalEdge.end) === '[51,9.5]',
            'goal stroke extends upward inside the existing box');
        check(goalEdge.start[1] - goalEdge.width / 2 === 8 &&
            goalEdge.start[1] + goalEdge.width / 2 === bar.height,
            'three-pixel goal marker grows inward without moving its bottom boundary');
        check(JSON.stringify(goalEdge.color) === JSON.stringify([168/255,85/255,247/255,.8]),
            'goal bottom uses goal purple and existing outline alpha');
        check(bar.draws[0].color[3] === .22 && bar.width === 52 && bar.height === 11,
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
print('PASS: goal bars keep the gradient and only a three-pixel purple bottom edge; focus and opacity remain intact.');

// Goal fill pulses independently of its steady purple marker.
const steadyGoal = {session_id: 'steady-goal', phase: 'RUNNING', run_active: true,
    goal_active: true, workflow: {available: true, fraction: .5}};
button._updatePanelProgress({providers: [{agents: [steadyGoal]}]}, false);
const steadyGoalBar = button._panelProgressBars.get('steady-goal');
for (const [now, expectedAlpha] of [[0,1], [750,166/255], [1500,77/255], [3000,1]]) {
    button._applyPanelProgressPulse(now);
    check(steadyGoalBar.opacity === 255, 'goal host remains fully opaque throughout pulse');
    check(steadyGoalBar.strokes.at(-1).color[3] === .8,
        'purple goal stripe keeps constant effective alpha');
    check(steadyGoalBar.strokes.at(-1).operator === imports.cairo.Operator.SOURCE,
        'purple replaces pulsing pixels beneath it instead of blending with them');
    check(steadyGoalBar.draws.at(-1).color[3] === expectedAlpha &&
        steadyGoalBar.draws.at(-2).color[3] === .22*expectedAlpha,
        'goal track and fill retain their previous effective opacity pulse');
}
button._updatePanelProgress({providers: [{agents: [steadyGoal]}]}, true);
check(steadyGoalBar.opacity === 128 && timers.size === 0, 'stale goal stays steadily dim without animation');
steadyGoal.goal_active = false;
button._updatePanelProgress({providers: [{agents: [steadyGoal]}]}, false);
button._applyPanelProgressPulse(1500);
check(steadyGoalBar.opacity === 77, 'ordinary run resumes compositor pulse on same actor');
button._updatePanelProgress(null, false);
print('PASS: purple goal stripe remains steady while progress opacity still pulses.');

// Exercise the shipped menu palette, including focus/goal/reset metadata.
const phaseOwners = new Function('print', `${source.slice(source.indexOf('const PHASE_COLORS ='), source.indexOf('};', source.indexOf('const PHASE_COLORS =')) + 2)}
    ${declaration('phaseColor')}
    ${declaration('agentPhaseBadgeColor')}
    return {phaseColor, agentPhaseBadgeColor};`)(print);
for (const phase of ['INTERRUPTED', 'interrupted', ' INTERRUPTED '])
    check(phaseOwners.phaseColor(phase) === '#ef4444', 'Interrupted panel/menu status must be red');
for (const focused of [false, true]) {
    const interrupted = {phase: 'INTERRUPTED', focused, goal_active: true,
        queued_reset_at: new Date(Date.now() + 60000).toISOString()};
    check(phaseOwners.agentPhaseBadgeColor(interrupted) === '#ef4444',
        'A reset countdown or goal must not recolor interrupted menu status');
}
check(phaseOwners.phaseColor('RUNNING') === '#e6c200', 'Running stays yellow');
check(phaseOwners.phaseColor('unknown-phase') === '#7f8ea3', 'Unknown stays muted');

// Interruption must also color the actual taskbar fill, at every progress fraction.
for (const fraction of [null, 0, .5, 1]) {
    for (const focused of [false, true]) {
        const agent = {phase: 'INTERRUPTED', focused, goal_active: true, run_active: false,
            workflow: {available: fraction !== null, fraction}, unread_completion: false};
        const bar = owners.buildPanelTaskProgress(agent);
        bar._amsPulseAt = 1500;
        bar.queue_repaint();
        const fills = bar.draws.filter(item => item.rect[0] === 1 && item.rect[1] === 1);
        check(fills.length > 0 && fills.every(item =>
            JSON.stringify(item.color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[239,68,68]'),
            `Interrupted track/fill must be red with fraction=${fraction} focused=${focused}`);
    }
}
print('PASS: interrupted taskbar progress fills stay red at every fraction.');

// Monitor boundaries preserve terminal ordering and double the ordinary six-pixel gap.
const monitors = [
    {session_id: 'm1-a', workspace: 2, window_display_id: 1, monitor: 0},
    {session_id: 'm1-b', workspace: 2, window_display_id: 2, monitor: 0},
    {session_id: 'm3-a', workspace: 2, window_display_id: 3, monitor: 2},
    {session_id: 'm3-b', workspace: 2, window_display_id: 4, monitor: 2},
    {session_id: 'ws5', workspace: 5, window_display_id: 1, monitor: 2},
];
const monitorPayload = {providers: [{kind: 'codex', agents: monitors}]};
button._updatePanelProgress(monitorPayload, false);
let monitorGroup = button._panelProgress.children[0].children[1];
check(JSON.stringify(monitorGroup.children.map(child => child.style_class)) === JSON.stringify([
    'ams-panel-task-cell', 'ams-panel-task-cell', 'ams-panel-monitor-divider',
    'ams-panel-task-cell', 'ams-panel-task-cell']), 'teal divider only at monitor boundary');
let monitorDivider = monitorGroup.children[2];
check(monitorDivider.width === 1,
    'two six-pixel layout gaps double the spacing around a one-pixel monitor line');
check(monitorDivider.y_align === Clutter.ActorAlign.FILL && monitorDivider.y_expand === true &&
    monitorDivider.height === undefined && !monitorDivider.reactive,
    'monitor line fills available panel height without a fixed-height constraint or click target');
monitorDivider.height = 36;
monitorDivider.queue_repaint();
check(JSON.stringify(monitorDivider.color) === JSON.stringify([0, 1, 230 / 255, .8]) &&
    monitorDivider.lineWidth === 1 && monitorDivider.dash === undefined &&
    JSON.stringify(monitorDivider.start) === '[0.5,9]' && JSON.stringify(monitorDivider.end) === '[0.5,27]',
    'solid one-pixel teal line spans the centered half of allocated height');
for (const height of [12, 50, 72]) {
    monitorDivider.height = height;
    monitorDivider.queue_repaint();
    check(monitorDivider.start[1] === height / 4 && monitorDivider.end[1] === height * 3 / 4,
        'teal monitor line always remains centered at half the panel height');
}
button._updatePanelProgress(monitorPayload, false);
check(button._panelProgress.children[0].children[1] === monitorGroup, 'unchanged monitor groups reuse actors');
monitors[2].monitor = 0;
monitors[3].monitor = 0;
button._updatePanelProgress(monitorPayload, false);
monitorGroup = button._panelProgress.children[0].children[1];
check(monitorGroup.children.length === 4 && monitorGroup.children.every(child => child.style_class === 'ams-panel-task-cell'),
    'moving monitors rebuilds the boundaries even when terminal IDs and workspace are unchanged');
monitors[1].monitor = null;
monitors[2].monitor = undefined;
monitors[3].monitor = null;
button._updatePanelProgress(monitorPayload, false);
check(button._panelProgress.children[0].children[1].children.filter(child => child.style_class === 'ams-panel-monitor-divider').length === 1,
    'unknown monitor values share one fallback group');
check(button._panelProgress.children[2].children[1].children.length === 1, 'no boundary at a single-monitor workspace edge');
check(JSON.stringify(visibleBars(button._panelProgress).map(bar => bar._amsAgent.session_id)) ===
    '["m1-a","m1-b","m3-a","m3-b","ws5"]', 'monitor boundaries keep the public terminal ID order');
button._updatePanelProgress(null, false);
check(button._panelProgress.children.length === 0 && button._panelProgressBars.size === 0, 'empty payload clears bars and monitor lines');
print('PASS: half-height centered teal monitor boundaries, doubled gaps, stable refresh and monitor moves.');

// Labels precede the bar row inline, without changing the boundary drawing allocation.
const labelAgents = [
    {session_id: 'label-w8', workspace: 8, window_display_id: 1, monitor: 0},
    {session_id: 'label-w2-a', workspace: 2, window_display_id: 1, monitor: 0},
    {session_id: 'label-w2-b', workspace: 2, window_display_id: 2, monitor: 2},
    {session_id: 'label-unknown', workspace: 0, monitor: null},
];
const labelPayload = {providers: [{kind: 'codex', agents: labelAgents}]};
button._updatePanelProgress(labelPayload, false);
let labelGroups = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(JSON.stringify(labelGroups.map(group => group.children[0].text)) === '["W2","W8","W?"]',
    'one label for each workspace, numeric order, honest unknown fallback');
for (const group of labelGroups) {
    check(group.vertical !== true && !group.reactive && group.children.length === 2,
        'workspace label precedes its bars in a horizontal group without a click target');
    const [label, row] = group.children;
    check(row.style_class === 'ams-panel-workspace' && row.y_expand === true && row.y_align === Clutter.ActorAlign.FILL,
        'bar row and monitor lines retain full panel height');
    check(label.style_class === 'ams-panel-workspace-label' && label.x_align === Clutter.ActorAlign.START &&
        label.y_align === Clutter.ActorAlign.CENTER && !label.reactive,
        'small workspace label sits inline at the left of its bars');
}
const retainedLabelGroup = labelGroups[0];
const retainedLabel = retainedLabelGroup.children[0];
button._updatePanelProgress(labelPayload, true);
check(button._panelProgress.children[0] === retainedLabelGroup && retainedLabelGroup.children[0] === retainedLabel,
    'progress-only and staleness refreshes retain labels and bars');
labelAgents[1].workspace = 3;
labelAgents[2].workspace = 3;
button._updatePanelProgress(labelPayload, false);
labelGroups = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(labelGroups[0].children[0].text === 'W3', 'workspace moves update the label');
button._updatePanelProgress(null, false);
check(button._panelProgress.children.length === 0, 'empty snapshot removes labels with its bars');
print('PASS: inline workspace labels before bars, preserved boundaries and stable refreshes.');

// Every rendered workspace label samples the same fixed blue-to-violet scale.
const workspaceColors = ['#60a5fa', '#6ea0fa', '#7b9cfb', '#8997fb',
    '#9792fb', '#a58dfb', '#b289fc', '#c084fc'];
const gradientAgents = Array.from({length: 8}, (_value, index) =>
    ({session_id: `gradient-${index + 1}`, workspace: index + 1, monitor: 0}));
button._updatePanelProgress({providers: [{kind: 'codex', agents: gradientAgents}]}, false);
const gradientGroups = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(gradientGroups.length === 8, 'render all eight workspace labels');
for (const [index, group] of gradientGroups.entries()) {
    const label = group.children[0];
    check(label.text === `W${index + 1}` && label.style === `color: ${workspaceColors[index]};`,
        'exact endpoint and intermediate gradient colors are applied to actual workspace labels');
}
button._updatePanelProgress({providers: [{agents: [
    {session_id: 'gradient-unknown', workspace: 0}, {session_id: 'gradient-high', workspace: 9},
]}]}, false);
const edgeLabels = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group').map(group => group.children[0]);
check(edgeLabels[0].text === 'W9' && edgeLabels[0].style === 'color: #c084fc;', 'workspace above scale keeps violet endpoint');
check(edgeLabels[1].text === 'W?' && edgeLabels[1].style === 'color: #9a9996;', 'unknown workspace has neutral color');
button._updatePanelProgress(null, false);
print('PASS: W1 blue → W8 violet gradient applies to every label, with stable endpoints.');

// The leading workspace label follows the existing provider/token summary.
const tokenCounter = new Actor({text: '64/s', style_class: 'ams-rate'});
const inlineButton = Object.assign({_box: new Actor()}, owners.updater);
inlineButton._box.add_child(tokenCounter);
inlineButton._updatePanelProgress({providers: [{kind: 'codex', agents: [
    {session_id: 'inline-w2', workspace: 2, monitor: 0},
    {session_id: 'inline-w5', workspace: 5, monitor: 0},
]}]}, false);
check(inlineButton._box.children[0] === tokenCounter && inlineButton._box.children[1] === inlineButton._panelProgress,
    'workspace strip follows token counter in the real panel update path');
const inlineGroup = inlineButton._panelProgress.children[0];
check(inlineGroup.children[0].text === 'W2' && inlineGroup.children[1].children[0].children[0]._amsAgent.session_id === 'inline-w2',
    'W2 is between the token counter and first bar');
check(inlineButton._panelProgress.children[2].children[0].text === 'W5', 'later workspace labels also precede their groups');
inlineButton._updatePanelProgress(null, false);
check(inlineButton._box.children[0] === tokenCounter, 'removing workspaces preserves the token counter');
print('PASS: token counter → W2 → first bar, followed by labeled workspace groups.');

// The shipped workspace typography is exactly 1.5 times its previous nine pixels.
const [, stylesheetBytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/stylesheet.css`).load_contents(null);
const stylesheet = new TextDecoder().decode(stylesheetBytes);
const workspaceLabelStyle = stylesheet.match(/\.ams-panel-workspace-label\s*\{([^}]+)\}/)?.[1] || '';
check(/font-size:\s*13\.5px;/.test(workspaceLabelStyle), 'workspace labels grow from 9px to 13.5px');
print('PASS: shipped workspace labels are 1.5x larger.');

// Running time belongs to its own agent, below that agent's unchanged drawing area.
const estimateAgents = [
    {session_id: 'estimate-a', workspace: 2, monitor: 0, window_display_id: 1, phase: 'RUNNING', run_active: true,
        task_time_estimate: {status: 'estimated', label: '~2m left'}, workflow: {fraction: .3}},
    {session_id: 'estimate-b', workspace: 2, monitor: 0, window_display_id: 2, phase: 'RUNNING', run_active: true,
        task_time_estimate: {status: 'estimated', label: '~1h 1m left'}, workflow: {fraction: .4}},
    {session_id: 'estimate-idle', workspace: 2, monitor: 0, window_display_id: 3, phase: 'DONE', run_active: false,
        task_time_estimate: {status: 'estimated', label: '~9m left'}, workflow: {fraction: 1}},
];
const estimatePayload = {providers: [{kind: 'codex', agents: estimateAgents}]};
button._updatePanelProgress(estimatePayload, false);
const estimateRow = button._panelProgress.children[0].children[1];
const estimateCells = estimateRow.children;
check(estimateCells.every(cell => cell.style_class === 'ams-panel-task-cell' && cell.width === 52 &&
    cell.layout_manager instanceof Clutter.BinLayout && cell.y_expand === true && cell.y_align === Clutter.ActorAlign.FILL && !cell.reactive),
    'each bar/time cell retains the drawing width and full-height panel allocation');
check(JSON.stringify(estimateCells.map(cell => cell.children[1].text)) === '["~2m","~1h 1m",""]',
    'each running bar gets its own compact forecast, completed bar gets none');
for (const cell of estimateCells) {
    const [bar, estimate] = cell.children;
    check(bar.style_class === 'ams-panel-progress-bar' && bar.width === 52 && bar.height === 11,
        'per-agent estimates preserve existing bar dimensions and pulse owner');
    check(estimate.style_class === 'ams-panel-agent-estimate' && estimate.x_align === Clutter.ActorAlign.CENTER &&
        estimate.y_align === Clutter.ActorAlign.END && estimate.x_expand === true && estimate.y_expand === true && !estimate.reactive,
        'forecast is centered below its own bar, independent of bar opacity');
}
const retainedEstimateCell = estimateCells[0];
const retainedEstimateBar = retainedEstimateCell.children[0];
const retainedEstimateLabel = retainedEstimateCell.children[1];
estimateAgents[0].task_time_estimate.label = '~1m left';
button._updatePanelProgress(estimatePayload, false);
check(estimateRow.children[0] === retainedEstimateCell && retainedEstimateCell.children[0] === retainedEstimateBar &&
    retainedEstimateCell.children[1] === retainedEstimateLabel && retainedEstimateLabel.text === '~1m',
    'estimate refresh retains cell, bar and label actors');
button._applyPanelProgressPulse(1500);
check(retainedEstimateBar.opacity === 77 && retainedEstimateLabel.opacity === 255, 'forecast stays readable while running bar pulses');
button._updatePanelProgress(estimatePayload, true);
check(retainedEstimateLabel.opacity === 128 && retainedEstimateLabel.visible, 'stale estimate dims without removing it');
for (const state of [
    {phase: 'RUNNING', run_active: false, task_time_estimate: {status: 'estimated', label: '~9m left'}},
    {phase: 'STOPPED', run_active: true, task_time_estimate: {status: 'estimated', label: '~9m left'}},
    {phase: 'RATE', run_active: false, queued_reset_at: '2099-01-01T00:00:00Z', task_time_estimate: {status: 'estimated', label: '~9m left'}},
    {phase: 'RUNNING', run_active: true, task_time_estimate: null},
    {phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'learning', label: 'learning time estimates'}},
    {phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'overrun', label: 'past estimate'}},
    {phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated', label: ''}},
    {phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated', label: 3}},
    {phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated'}},
]) {
    Object.assign(estimateAgents[0], state);
    button._updatePanelProgress(estimatePayload, false);
    check(retainedEstimateLabel.text === '' && !retainedEstimateLabel.visible, 'no estimate for inactive or unavailable forecast state');
    check(estimateCells[1].children[1].text === '~1h 1m' && estimateCells[1].children[1].visible,
        'one missing forecast never changes another running agent estimate');
}
Object.assign(estimateAgents[0], {phase: 'RUNNING', run_active: true,
    task_time_estimate: {status: 'estimated', label: '  ~4m left  '}});
button._updatePanelProgress(estimatePayload, false);
check(retainedEstimateLabel.text === '~4m' && retainedEstimateLabel.visible && retainedEstimateLabel.opacity === 255,
    'fresh estimate recovers on existing actor');
snapshot = estimatePayload;
button.menu.isOpen = true;
button._updateTaskRows = () => {};
button._refresh();
check(estimateRow.children[0] === retainedEstimateCell && retainedEstimateLabel.text === '~4m', 'open menu refresh preserves per-bar time actors');
estimateAgents[0].task_time_estimate.label = '~3m left';
button._refresh();
check(retainedEstimateLabel.text === '~3m', 'open menu refresh updates per-bar time');
button.menu.isOpen = false;
button._refresh();
check(button._panelProgress.children[0].children[1].children[0] === retainedEstimateCell,
    'closing menu does not recreate the estimate cell');
button._updatePanelProgress({providers: [{agents: [estimateAgents[1]]}]}, false);
check(visibleBars(button._panelProgress).length === 1 && button._panelProgressBars.has('estimate-b') &&
    !button._panelProgressBars.has('estimate-a'), 'closed agent removes its forecast and leaves the remaining bar');
button._updatePanelProgress(null, false);
check(button._panelProgress.children.length === 0 && button._panelProgressBars.size === 0, 'missing snapshot removes bars and forecast labels');
print('PASS: independent running estimates below bars, retained actors, state transitions and menu refreshes.');

// Draft tracks and measured fills use the same lime as the visible DRAFT badge.
for (const phase of ['DRAFT', ' draft ']) {
    for (const fraction of [0, 0.25, 0.5, 1]) {
        const draft = {session_id: 'draft-color', phase, run_active: false,
            workflow: {available: true, fraction}};
        check(owners.agentPhaseBadgeColor(draft, 0) === '#84c32f', 'draft menu badge retains lime');
        for (let refresh = 0; refresh < 2; refresh++) {
            button._updatePanelProgress({providers: [{agents: [draft]}]}, false);
            button._applyPanelProgressPulse(1500);
            const bar = button._panelProgressBars.get('draft-color');
            const track = bar.draws.at(fraction > 0 ? -2 : -1);
            const fill = bar.draws.at(-1);
            for (const paint of [track, fill]) {
                const rgb = paint.color.slice(0, 3).map(channel => Math.round(channel * 255));
                check(JSON.stringify(rgb) === '[132,195,47]',
                    `DRAFT bar matches lime menu color at fraction ${fraction}, got ${rgb}`);
            }
            check(track.color[3] === .22 && track.rect[2] === 50, 'draft retains faint full-width track');
            check(fill.rect[2] === (fraction > 0 ? 50 * fraction : 50), 'draft preserves measured fill width');
            check(fill.color[3] === (fraction > 0 ? 1 : .22), 'zero-progress draft invents no opaque fill');
            check(bar.opacity === 255 && timers.size === 0, 'draft stays steady without running or ready pulses');
        }
        button._updatePanelProgress({providers: [{agents: [draft]}]}, true);
        const staleDraft = button._panelProgressBars.get('draft-color');
        check(staleDraft.opacity === 128 && timers.size === 0, 'stale draft keeps normal dimming');
        check(JSON.stringify(staleDraft.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[132,195,47]',
            'staleness retains the draft hue');
        button._updatePanelProgress(null, false);
        check(button._panelProgressBars.size === 0, 'draft case cleanup removes the bar');
    }
}
const draftTransition = {session_id: 'draft-transition', phase: 'DRAFT', workflow: {available: true, fraction: .5}};
button._updatePanelProgress({providers: [{agents: [draftTransition]}]}, false);
const draftTransitionBar = button._panelProgressBars.get('draft-transition');
for (const [phase, run_active, expected] of [
    ['RUNNING', true, [137,133,81]], ['DRAFT', false, [132,195,47]],
    ['DONE', false, [137,133,81]], ['DRAFT', false, [132,195,47]],
]) {
    Object.assign(draftTransition, {phase, run_active});
    button._updatePanelProgress({providers: [{agents: [draftTransition]}]}, false);
    check(button._panelProgressBars.get('draft-transition') === draftTransitionBar, 'phase changes reuse draft actor');
    check(JSON.stringify(draftTransitionBar.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255))) === JSON.stringify(expected),
        'leaving draft restores the ordinary progress gradient');
}
draftTransition.workflow.available = false;
button._updatePanelProgress({providers: [{agents: [draftTransition]}]}, false);
check(JSON.stringify(draftTransitionBar.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[48,52,64]',
    'unavailable draft progress retains neutral track');
button._updatePanelProgress(null, false);
check(timers.size === 0, 'draft transitions clean up animation');
print('PASS: DRAFT bars match lime menu badges, retaining measured widths, alpha, stale dimming and phase transitions.');
// No aggregate estimate actor remains at the right, including hidden or empty states.
snapshot = estimatePayload;
button.menu.isOpen = false;
button._refresh();
check(button._box.children.every(child => child.style_class !== 'ams-panel-estimate'), 'far-right aggregate estimate actor is completely removed');
const localEstimateCell = button._panelProgress.children[0].children[1].children[0];
check(localEstimateCell.children[1].text === '~3m', 'removing aggregate keeps the running agent estimate');
for (const isOpen of [true, false]) {
    button.menu.isOpen = isOpen;
    for (const value of [estimatePayload, null, {providers: []}, estimatePayload]) {
        snapshot = value;
        button._refresh();
        check(button._box.children.every(child => child.style_class !== 'ams-panel-estimate'),
            'open/closed menu and missing/empty/recovered payload never recreate trailing estimate');
    }
}
check(visibleBars(button._panelProgress).some(bar => bar._amsEstimateLabel.text === '~3m'), 'own-agent forecasts survive refresh and recovery');
print('PASS: trailing estimate removed in every refresh state; per-bar forecast remains.');

// Runtime is a small HH:MM clock above the bar, including the recorded final run.
const savedDateNow = Date.now;
const runtimeReference = Date.parse('2026-10-13T10:00:00Z');
Date.now = () => runtimeReference;
const runtimeAgents = [
    {session_id: 'runtime-live', workspace: 2, monitor: 0, window_display_id: 1, phase: 'RUNNING', run_active: true,
        run_started_at: new Date(runtimeReference - 3660 * 1000).toISOString(), workflow: {fraction: .5},
        task_time_estimate: {status: 'estimated', label: '~3m left', observed_active_seconds: 20}},
    {session_id: 'runtime-done', workspace: 2, monitor: 0, window_display_id: 2, phase: 'DONE', run_active: false,
        run_started_at: '', ready_completed_at: '2026-10-13T09:00:00Z', workflow: {fraction: 1},
        task_time_estimate: {status: 'complete', observed_active_seconds: 7865}},
];
const runtimePayload = {providers: [{kind: 'codex', agents: runtimeAgents}]};
button._updatePanelProgress(runtimePayload, false);
const runtimeRow = button._panelProgress.children[0].children[1];
const runtimeCells = runtimeRow.children;
check(runtimeCells.every(cell => cell.children.length === 3), 'each cell contains bar, lower forecast and upper runtime');
const liveRuntime = runtimeCells[0].children[2];
const doneRuntime = runtimeCells[1].children[2];
check(liveRuntime.text === '01:01' && doneRuntime.text === '02:11', 'live run start and measured completed duration both format hours:minutes');
for (const cell of runtimeCells) {
    const runtime = cell.children[2];
    check(runtime.style_class === 'ams-panel-agent-runtime' && runtime.x_align === Clutter.ActorAlign.CENTER &&
        runtime.y_align === Clutter.ActorAlign.START && runtime.x_expand === true && runtime.y_expand === true && !runtime.reactive,
        'runtime sits at the top centered above the retained bar');
}
check(runtimeCells[0].children[1].text === '~3m' && !runtimeCells[1].children[1].visible,
    'running forecast remains below while completed runtime has no remaining estimate');
button._applyPanelProgressPulse(runtimeReference + 60 * 1000);
check(liveRuntime.text === '01:02' && doneRuntime.text === '02:11', 'live minute ticks without replacing actors or advancing final duration');
check(liveRuntime.opacity === 255 && doneRuntime.opacity === 255, 'runtime labels do not pulse with progress fill');
for (const [seconds, expected] of [[0, '00:00'], [59, '00:00'], [60, '00:01'], [3599, '00:59'],
    [3600, '01:00'], [3660, '01:01'], [90000, '25:00'], [360000, '100:00']]) {
    runtimeAgents[0].run_started_at = new Date(runtimeReference - seconds * 1000).toISOString();
    runtimeAgents[1].task_time_estimate.observed_active_seconds = seconds;
    button._updatePanelProgress(runtimePayload, false);
    check(liveRuntime.text === expected && doneRuntime.text === expected, 'HH:MM truncates partial minutes and keeps hours beyond a day');
}
runtimeAgents[0].run_started_at = new Date(runtimeReference + 60000).toISOString();
button._updatePanelProgress(runtimePayload, false);
check(liveRuntime.text === '00:00', 'future live timestamp never produces negative runtime');
for (const value of [null, -1, NaN, Infinity, '120']) {
    runtimeAgents[1].task_time_estimate.observed_active_seconds = value;
    button._updatePanelProgress(runtimePayload, false);
    check(doneRuntime.text === '' && !doneRuntime.visible, 'unavailable or invalid completed duration is omitted honestly');
}
for (const phase of ['DRAFT', 'STOPPED', 'ATTN', 'RATE', 'INTERRUPTED']) {
    runtimeAgents[1].phase = phase;
    runtimeAgents[1].task_time_estimate.observed_active_seconds = 300;
    button._updatePanelProgress(runtimePayload, false);
    check(doneRuntime.text === '' && !doneRuntime.visible, 'nonrunning unfinished states do not invent a completed runtime');
}
Object.assign(runtimeAgents[1], {phase: 'DONE', run_started_at: '2026-10-13T07:45:00Z',
    ready_completed_at: '2026-10-13T09:00:00Z', task_time_estimate: null});
button._updatePanelProgress(runtimePayload, false);
check(doneRuntime.text === '01:15', 'timestamp-backed completed records without measured duration retain their final clock');
runtimeAgents[1].ready_completed_at = 'invalid';
button._updatePanelProgress(runtimePayload, false);
check(doneRuntime.text === '' && !doneRuntime.visible, 'malformed completion timestamp is omitted');
Object.assign(runtimeAgents[0], {run_started_at: 'invalid', task_time_estimate: {status: 'estimated', observed_active_seconds: 300, label: '~2m left'}});
button._updatePanelProgress(runtimePayload, false);
check(liveRuntime.text === '00:05', 'published active duration backs a missing live start');
runtimeAgents[0].task_time_estimate.observed_active_seconds = null;
button._updatePanelProgress(runtimePayload, false);
check(liveRuntime.text === '' && !liveRuntime.visible, 'unknown live duration stays blank');
Object.assign(runtimeAgents[0], {run_started_at: new Date(runtimeReference - 3660 * 1000).toISOString(),
    task_time_estimate: {status: 'estimated', label: '~3m left'}});
button._updatePanelProgress(runtimePayload, true);
check(liveRuntime.text === '01:01' && liveRuntime.opacity === 128, 'stale clock keeps last value and dims');
button._applyPanelProgressPulse(runtimeReference + 120 * 1000);
check(liveRuntime.text === '01:01', 'stale runtime does not accrue time from a stale snapshot');
Date.now = () => runtimeReference + 120 * 1000;
button._updatePanelProgress({...runtimePayload, updatedAt: new Date(runtimeReference).toISOString()}, true);
check(liveRuntime.text === '01:01', 'repeated stale payload refresh uses publication time instead of continuing clock');
Date.now = () => runtimeReference;
Object.assign(runtimeAgents[0], {phase: 'DONE', run_active: false, run_started_at: '',
    task_time_estimate: {status: 'complete', observed_active_seconds: 3720}});
button._updatePanelProgress(runtimePayload, false);
check(runtimeRow.children[0] === runtimeCells[0] && liveRuntime.text === '01:02' && liveRuntime.visible && liveRuntime.opacity === 255 &&
    !runtimeCells[0].children[1].visible, 'finishing run freezes measured runtime on same actor and clears remaining estimate');
button._applyPanelProgressPulse(runtimeReference + 3600000);
check(liveRuntime.text === '01:02', 'completed runtime remains fixed on later animation ticks');
// Rebuild with the same completed payload, as after an extension reload.
button._updatePanelProgress(null, false);
button._updatePanelProgress(runtimePayload, false);
check(button._panelProgress.children[0].children[1].children[0].children[2].text === '01:02',
    'completed runtime restores from published measurement without an in-memory clock cache');
snapshot = runtimePayload;
button.menu.isOpen = true;
button._refresh();
check(button._panelProgress.children[0].children[1].children[0].children[2].text === '01:02', 'open-menu refresh preserves completed HH:MM runtime');
button.menu.isOpen = false;
button._updatePanelProgress(null, false);
Date.now = savedDateNow;
print('PASS: HH:MM runtime above every known live/completed bar, minute ticks, final freeze and reload recovery.');
// Exercise real retained taskbar cells using the same selector action as menu rows.
Clutter.EVENT_STOP = 1;
Clutter.EVENT_PROPAGATE = 0;
const selectedNotes = [];
button._focusAgent = noteId => selectedNotes.push(noteId);
const selectionAgents = [
    {session_id: 'select-a', note_id: '  note-a  ', workspace: 2, monitor: 0, phase: 'DRAFT'},
    {session_id: 'select-b', note_id: 'note-b', workspace: 5, monitor: 1, phase: 'DONE'},
    {session_id: 'select-c', note_id: 'note-c', workspace: 5, monitor: 1, phase: 'RUNNING', run_active: true},
    {session_id: 'select-unlinked', note_id: '', workspace: 5, monitor: 1, phase: 'DONE'},
];
const selectionPayload = {providers: [{kind: 'codex', agents: selectionAgents}]};
button._updatePanelProgress(selectionPayload, false);
const selectionCell = key => button._panelProgressBars.get(key).get_parent();
for (const key of ['select-a', 'select-b', 'select-c']) {
    const cell = selectionCell(key);
    check(cell.reactive && cell.can_focus, 'linked instance has its own interactive cell');
    check(cell.children.every(child => child.reactive === false), 'progress and forecast route picking to their instance cell');
    check(cell['button-press-event'](cell, {get_button: () => 1}) === Clutter.EVENT_STOP,
        'individual left-click is consumed before the enclosing panel menu');
}
check(JSON.stringify(selectedNotes) === '["note-a","note-b","note-c"]', 'draft, done and running instances select their own exact note');
const retainedSelectionCell = selectionCell('select-b');
button._updatePanelProgress({providers: [{kind: 'codex', agents: selectionAgents.map(agent =>
    agent.session_id === 'select-b' ? {...agent, note_id: 'note-b-new'} : agent)}]}, true);
check(selectionCell('select-b') === retainedSelectionCell, 'refresh retains selection actor');
retainedSelectionCell['button-press-event'](retainedSelectionCell, {get_button: () => 1});
check(selectedNotes.at(-1) === 'note-b-new', 'click reads current identity after retained/stale refresh');
for (const mouseButton of [2, 3, 8]) {
    const before = selectedNotes.length;
    check(retainedSelectionCell['button-press-event'](retainedSelectionCell, {get_button: () => mouseButton}) === Clutter.EVENT_PROPAGATE,
        'other mouse buttons do not invoke instance selection');
    check(selectedNotes.length === before, 'rejected selection leaves prior focus calls unchanged');
    check(selectionCell('select-b') === retainedSelectionCell, 'rejected selection preserves rendered cells');
}
check(selectionCell('select-unlinked').reactive === false, 'no selection target means no misleading clickable cell');
button._updatePanelProgress({providers: [{agents: selectionAgents.map(agent => ({...agent, note_id: ''}))}]}, false);
check(retainedSelectionCell.reactive === false, 'retained cell stops accepting clicks when note identity disappears');
button._updatePanelProgress(null, false);
check(retainedSelectionCell.destroyed && button._panelProgressBars.size === 0, 'closing instances destroys their click targets');
print('PASS: individual instance left-click selection, current identity, all phases, other-button rejection and cleanup.');

// Hover belongs to each retained instance cell, never to its progress/clock children.
button._updatePanelProgress(selectionPayload, false);
const hoverCells = ['select-a', 'select-b', 'select-c'].map(selectionCell);
check(hoverCells.every(cell => cell.track_hover === true), 'each selectable instance tracks its own hover');
check(hoverCells.every(cell => cell.children.every(child => !child.reactive && !child.track_hover)),
    'status bar, runtime and forecast route hover to their exact instance');
const hoverNeighbor = hoverCells[1];
hoverCells[0].hover = true;
check(!hoverNeighbor.hover && hoverCells[0].width === 52 && hoverNeighbor.width === 52,
    'hover does not spread to the adjacent instance or change its geometry');
button._updatePanelProgress(selectionPayload, false);
check(selectionCell('select-a') === hoverCells[0] && hoverCells[0].hover === true,
    'status refresh retains hovered instance and its local highlight');
button._updatePanelProgress(null, false);
print('PASS: independent hover targets, inert text/paint children, retained hover and unchanged widths.');
