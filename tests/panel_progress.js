// Run the real taskbar rendering owners under gjs with isolated actor stubs.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes);

/** Extract a JavaScript owner for isolated execution.
 * Parameters: name - function or method name; method - whether it is a class method.
 * Returns: the declaration, adapting prototype callbacks for the isolated fixture object.
 */
function declaration(name, method = false) {
    print(`name=${name} method=${method}`);
    const marker = method ? `\n    ${name}(` : `\nfunction ${name}(`;
    const callbackMarker = method ? `\nAccModelStatusButton.prototype.${name} = function (` : `\nconst ${name} = (`;
    const callback = source.indexOf(marker) < 0;
    const start = source.indexOf(callback ? callbackMarker : marker);
    if (start < 0)
        throw Error(`Missing public rendering owner ${name}`);
    let depth = 0;
    for (let i = source.indexOf('{', start); i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}' && --depth === 0) {
            let result = source.slice(start + 1, i + 1);
            // The fixture object calls extracted class callbacks as dynamic methods.
            if (method && callback)
                result = result.replace(`AccModelStatusButton.prototype.${name} = function (`, `${name}(`);
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
    clutter_text = {};
    add_child = child => { child._parent = this; this.children.push(child); };
    remove_child = child => {
        // An actor leaving the stage loses hover throughout its subtree.
        const pending = [child];
        while (pending.length) {
            const actor = pending.pop();
            actor.hover = false;
            actor.stageDetachments = (actor.stageDetachments || 0) + 1;
            pending.push(...actor.children);
        }
        child._parent = null;
        this.children = this.children.filter(item => item !== child);
    };
    set_child_above_sibling = (child, sibling) => {
        child.restackings = (child.restackings || 0) + 1;
        this.children = this.children.filter(item => item !== child);
        const position = sibling ? this.children.indexOf(sibling) + 1 : this.children.length;
        this.children.splice(position, 0, child);
    };
    set_child_below_sibling = (child, sibling) => {
        child.restackings = (child.restackings || 0) + 1;
        this.children = this.children.filter(item => item !== child);
        const position = sibling ? this.children.indexOf(sibling) : 0;
        this.children.splice(position, 0, child);
    };
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
        this._parent?.remove_child(this);
        this.destroyed = true;
        this.destroySignal?.(this);
        print(undefined);
    }
    connect = (signal, callback) => { this[signal === 'destroy' ? 'destroySignal' : signal] = callback; return 1; };
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

const St = {Icon: Actor, BoxLayout: Actor, DrawingArea: Actor, Label: Actor, Widget: Actor};
const Clutter = {ModifierType: {CONTROL_MASK: 4}, ActorAlign: {CENTER: 0, FILL: 1, END: 2, START: 3}, BinLayout: class {}};
const snippets = ['phaseColor', 'agentPhaseBadgeColor', 'taskProgressValue', 'panelTaskProgressValue', 'panelProgressColor', 'panelProgressStateColor', 'panelProgressWave', 'panelProgressUnderway', 'panelProgressGoalActive', 'panelProgressOpacity', 'panelProgressHostOpacity', 'paintPanelTaskProgress', 'buildPanelTaskProgress',
    'workspaceSortValue', 'windowDisplaySortValue', 'monitorSortValue', 'compareWorkspaceEntries', 'groupWorkspaceEntries',
    'paintPanelWorkspaceSeparator', 'buildPanelWorkspaceSeparator', 'taskEstimateText',
    ...(source.includes('function buildPanelTaskCell(') ? ['panelAgentEstimateText', 'panelInstanceNumbers', 'buildPanelTaskCell'] : []),
    ...(source.includes('function exhaustedCodexAccounts(') ? ['readPanelQuotaSnapshot', 'exhaustedCodexAccounts'] : []),
    ...(source.includes('function panelResetRemainingSeconds(') ? ['panelResetRemainingSeconds'] : []),
    ...(source.includes('function taskMarkedDone(') ? ['taskMarkedDone'] : []),
    ...(source.includes('function displayedPhase(') ? ['displayedPhase'] : []),
    ...(source.includes('function panelLaunchWaitSeconds(') ? ['panelLaunchWaitSeconds'] : []),
    ...((source.includes('function panelAgentRuntimeText(') || source.includes('const panelAgentRuntimeText = (')) ? ['formatPanelRunDuration', 'panelAgentRuntimeText'] : []),
    ...((source.includes('function panelAgentRuntimeColor(') || source.includes('const panelAgentRuntimeColor = (')) ? ['panelAgentRuntimeColor'] : []),
    'panelCompletionAgeText',
    ...(source.includes('function panelWorkspaceColor(') ? ['panelWorkspaceColor'] : []),
    ...(source.includes('function buildPanelMonitorSeparator(') ? ['paintPanelMonitorSeparator', 'buildPanelMonitorSeparator'] : [])]
    .map(name => declaration(name));
const [, harborBytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/harbor.js`).load_contents(null);
const harborSource = new TextDecoder().decode(harborBytes).replace(/^import .*;$/gm, '')
    .replaceAll('export function', 'function').replaceAll('export const', 'const');
const Harbor = new Function(`${harborSource}; return {harborPanelEntries, harborCountText, formatHarborEstimate};`)();
const timers = new Map();
let nextTimer = 1;
const GLib = {PRIORITY_DEFAULT: 0, SOURCE_CONTINUE: true,
    timeout_add: (_priority, delay, callback) => {
        const id = nextTimer++;
        callback.delay = delay;
        timers.set(id, callback);
        return id;
    },
    source_remove: id => timers.delete(id),
};
let snapshot = null;
const services = {readPayload: () => snapshot, scopePayloadToWindows: value => value,
    withWindowDisplayIds: value => value, readWindowDisplayIds: () => new Map(),
    ageSeconds: () => 0, STALE_SECONDS: 180, unreadCompletionCount: () => 0,
    saveUnreadStates: () => {}, log: () => {}, workspaceAttentionCounts: () => [{workspace: 5, count: 1, current: false, label: 'W5:1'}], panelHarnessRateRows: () => [],
};
const makeOwners = new Function('St', 'Clutter', 'GLib', 'services', 'print', 'Cairo', 'Harbor', 'Gio', 'Pango', `${source.slice(source.indexOf('const PHASE_COLORS ='), source.indexOf('};', source.indexOf('const PHASE_COLORS =')) + 2)}
${snippets.join('\n')}
    const FALLBACK_ICON = 'acc-provider-generic-symbolic';
    const {harborCountText, formatHarborEstimate} = Harbor;
    const {readPayload, scopePayloadToWindows, withWindowDisplayIds, readWindowDisplayIds,
        ageSeconds, STALE_SECONDS,
        unreadCompletionCount, saveUnreadStates, log, workspaceAttentionCounts, panelHarnessRateRows} = services;
    class Destroyer extends St.BoxLayout { _hideTaskConfirmation = () => {}; ${declaration('_hideTaskHover', true)} ${declaration('destroy', true)} }
    return {agentPhaseBadgeColor, panelTaskProgressValue, panelProgressUnderway, panelProgressOpacity, paintPanelTaskProgress, buildPanelTaskProgress,
        Destroyer, paintPanelWorkspaceSeparator,
        updater: {${['_hideTaskHover', '_refresh', '_icon', '_updatePanelProgress', '_syncPanelPulseTimer', '_applyPanelProgressPulse',
            ...(source.includes('\n    _updatePanelTaskEstimate(') ? ['_updatePanelTaskEstimate'] : []),
            ...(source.includes('\n    _renderWorkspaceAttention(') ? ['_renderWorkspaceAttention'] : [])].map(name => declaration(name, true)).join(',')}}};`);
const owners = makeOwners(St, Clutter, GLib, services, print, imports.cairo, Harbor, Gio, imports.gi.Pango);
const check = (condition, message) => { if (!condition) throw Error(message); };
const visibleBars = container => container.children.flatMap(child =>
    child.style_class === 'ams-panel-workspace-group' ? child.children[1].children.filter(cell => cell.style_class === 'ams-panel-task-cell').map(cell => cell._amsBar) : []);
const a = {session_id: 'a', phase: 'RUNNING', run_active: true, workflow: {available: true, fraction: 0.25}};
const b = {session_id: 'b', phase: 'STOPPED', workflow: {available: true, fraction: 0.6}};
const payload = {providers: [{kind: 'codex', agents: [a]}, {kind: 'claude', agents: [b]}]};
const iconsDir = Gio.File.new_for_path(`${root}/acc-model-status@local/icons`);
const button = Object.assign({_iconsDir: iconsDir, _box: new Actor(), _syncTaskHover: () => {}}, owners.updater);
button._updatePanelProgress(payload, false);
let bars = visibleBars(button._panelProgress);
check(bars.length === 2, 'one bar for every open agent');
check(bars.every(bar => bar.width === 52 && bar.height === 11), '52x11 drawing room preserves the 50x8 fill and outside outline');
check(bars[0].draws.at(-1).rect[2] === 12.5, 'actual 25% fill');
check(bars[1].draws.at(-1).rect[2] === 30, 'actual 60% fill');
check(bars[1].draws.at(-1).color[3] === .72, 'partial fill must show the mark underneath');
const brandAgent = {session_id: 'branding', kind: 'codex', workflow: {fraction: 0}};
const brandProvider = {kind: 'codex', agents: [brandAgent]};
button._updatePanelProgress({providers: [brandProvider]}, false);
const brandBar = button._panelProgressBars.get('branding');
const brandCell = brandBar.get_parent();
const brandLogo = brandBar._amsLogo;
check(brandCell.children[0] === brandLogo &&
    brandCell.children[1] === brandBar &&
    brandCell.children[2] === brandBar._amsEstimateLabel.get_parent() &&
    brandCell.children[3] === brandBar._amsRuntimeLabel &&
    brandLogo.gicon.get_file().get_basename() === 'acc-provider-codex-symbolic.svg',
    'Codex artwork is behind the progress fill, footer and runtime');
check(brandCell.clip_to_allocation && brandLogo.icon_size === brandCell.width &&
    brandLogo.x_align === Clutter.ActorAlign.CENTER && brandLogo.y_align === Clutter.ActorAlign.CENTER &&
    !brandLogo.reactive && brandLogo.opacity > 0 && brandLogo.opacity < 128,
    'full-cell artwork spans the time labels while staying within the cell and subdued');
check(brandLogo.visible && brandBar.draws.at(-1).color[3] === .22, 'empty bar reveals the mark through the track');
brandAgent.kind = brandProvider.kind = 'vibe';
brandProvider.icon = 'acc-provider-generic-symbolic';
brandAgent.workflow = {available: false};
button._updatePanelProgress({providers: [brandProvider]}, false);
check(brandBar === button._panelProgressBars.get('branding') && brandCell.children[0] === brandLogo && brandLogo === brandBar._amsLogo,
    'harness change retains the cell, bar and logo actors');
check(brandLogo.gicon.get_file().get_basename() === 'acc-provider-vibe-symbolic.svg' &&
    brandLogo.visible && brandBar.draws.at(-1).color[3] < 1, 'Vibe resolves even with a legacy generic icon and unknown progress');
brandAgent.kind = brandProvider.kind = 'claude';
brandAgent.workflow = {fraction: 1};
button._updatePanelProgress({providers: [brandProvider]}, false);
check(brandLogo.gicon.get_file().get_basename() === 'acc-provider-claude-symbolic.svg' &&
    brandLogo.visible && brandBar.draws.at(-1).color[3] === 1, 'completion covers the middle of the logo with opaque fill while leaving the full-cell background');
brandAgent.kind = brandProvider.kind = 'future';
brandAgent.workflow = {fraction: .5};
button._updatePanelProgress({providers: [brandProvider]}, false);
check(brandLogo.visible && brandLogo.gicon.get_file().get_basename() === 'acc-provider-generic-symbolic.svg',
    'an unknown harness still has a fallback mark');
const freshLogoOpacity = brandLogo.opacity;
button._updatePanelProgress({providers: [brandProvider]}, true);
check(brandLogo.visible && brandLogo.opacity > 0 && brandLogo.opacity < freshLogoOpacity,
    'stale data dims the background without removing the artwork');
button._updatePanelProgress(payload, false);
bars = visibleBars(button._panelProgress);

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
check(timers.size === 1 && [...timers.values()][0].delay === 1000 && readyBar.opacity === 255,
    'acknowledged completion replaces animation with a steady second-by-second age clock');
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
    check(bar.draws[0].color[3] === .22 && fill.color[3] === (state.phase === 'DONE' ? 1 : .72),
        'state colors use translucent partial fill and opaque completion');
    check(fill.rect[2] === (state.phase === 'DONE' ? 50 : 25), 'state color preserves measured width');
}
for (const state of [{phase: 'RATE'}, {phase: 'RUNNING', run_active: true, goal_active: true}]) {
    const bar = owners.buildPanelTaskProgress({...state, workflow: {available: false}});
    bar._amsPulseAt = 0;
    bar.queue_repaint();
    check(bar.draws.length === 1 && bar.draws[0].color[3] === .45,
        'unavailable state keeps the neutral track translucent');
    check(JSON.stringify(bar.draws[0].color.slice(0, 3).map(channel => Math.round(channel * 255))) === '[48,52,64]',
        'unavailable workflow stays neutral');
}
print('PASS: goal and RATE fills match state colors and preserve measured progress.');

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
    check(painted.color[3] === (fraction === 1 ? 1 : fraction > 0 ? .72 : .22), 'ordinary gradient preserves alpha');
    check(painted.rect[2] === (fraction > 0 ? fraction * 50 : 50), 'ordinary gradient preserves track/fill width');
}
print('PASS: ordinary active bars transition red to green with measured widths and translucent partial fill.');

// Goal identity moves to the timer; progress keeps its measured gradient and focus outline.
for (const focused of [false, true]) {
    for (const [fraction, expected] of [[0, [239,68,68]], [.5, [137,133,81]], [1, [34,197,94]]]) {
        const agent = {phase: 'RUNNING', run_active: true, goal_active: true, focused,
            workflow: {available: true, fraction}};
        const bar = owners.buildPanelTaskProgress(agent);
        bar._amsPulseAt = 0;
        bar.queue_repaint();
        check(JSON.stringify(bar.draws[0].color.slice(0,3).map(value => Math.round(value*255))) === JSON.stringify(expected),
            'goal retains the measured progress gradient');
        check(bar.strokes.length === (focused ? 1 : 0), 'goal bar has only the optional teal focus outline');
        check(bar.strokes.every(stroke => stroke.width === 1 &&
            JSON.stringify(stroke.color) === JSON.stringify([0,1,230/255,.8])),
            'goal painting adds no purple stroke and preserves the teal focus edge');
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
print('PASS: goal bars keep gradient and teal focus outline with no purple partition.');

// Goal fill retains its previous pulse while the purple stripe is removed.
const steadyGoal = {session_id: 'steady-goal', phase: 'RUNNING', run_active: true,
    goal_active: true, workflow: {available: true, fraction: .5}};
button._updatePanelProgress({providers: [{agents: [steadyGoal]}]}, false);
const steadyGoalBar = button._panelProgressBars.get('steady-goal');
for (const [now, expectedAlpha] of [[0,1], [750,166/255], [1500,77/255], [3000,1]]) {
    button._applyPanelProgressPulse(now);
    check(steadyGoalBar.opacity === 255, 'goal host remains fully opaque throughout pulse');
    check(steadyGoalBar.strokes.length === 0, 'goal pulses never repaint a purple stripe');
    check(steadyGoalBar.draws.at(-1).color[3] === .72*expectedAlpha &&
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
print('PASS: goal stripe stays removed while progress opacity still pulses.');

// Exercise the shipped menu palette, including focus/goal/reset metadata.
const phaseOwners = new Function('print', `${source.slice(source.indexOf('const PHASE_COLORS ='), source.indexOf('};', source.indexOf('const PHASE_COLORS =')) + 2)}
    ${declaration('taskMarkedDone')}
    ${declaration('displayedPhase')}
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
const inlineButton = Object.assign({_iconsDir: iconsDir, _box: new Actor()}, owners.updater);
inlineButton._box.add_child(tokenCounter);
inlineButton._updatePanelProgress({providers: [{kind: 'codex', agents: [
    {session_id: 'inline-w2', workspace: 2, monitor: 0},
    {session_id: 'inline-w5', workspace: 5, monitor: 0},
]}]}, false);
check(inlineButton._box.children[0] === tokenCounter && inlineButton._box.children[1] === inlineButton._panelProgress,
    'workspace strip follows token counter in the real panel update path');
const inlineGroup = inlineButton._panelProgress.children[0];
check(inlineGroup.children[0].text === 'W2' && inlineGroup.children[1].children[0]._amsBar._amsAgent.session_id === 'inline-w2',
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
check(JSON.stringify(estimateCells.map(cell => cell._amsBar._amsEstimateLabel.text)) === '["~2m","~1h1m",""]',
    'each running bar gets its own compact forecast, completed bar gets none');
for (const cell of estimateCells) {
    const bar = cell._amsBar;
    const estimate = bar._amsEstimateLabel;
    check(bar.style_class === 'ams-panel-progress-bar' && bar.width === 52 && bar.height === 11,
        'per-agent estimates preserve existing bar dimensions and pulse owner');
    check(estimate.style_class === 'ams-panel-agent-estimate' && estimate.x_align === Clutter.ActorAlign.END &&
        estimate.y_align === Clutter.ActorAlign.END && estimate.x_expand === true && !estimate.reactive,
        'forecast is right-aligned below its own bar, independent of bar opacity');
}

const instanceStyle = stylesheet.match(/\.ams-panel-agent-instance\s*\{([^}]+)\}/)?.[1] || '';
check(/font-size:\s*13\.75px;/.test(instanceStyle), 'panel instance numbers grow from 11px to 13.75px');
check(/color:\s*#9ebfec;/.test(instanceStyle), 'the default panel instance number color stays blue');

// Account and project-local instance numbers share the footer with the estimate.
const numberedBase = {project: 'sample', workspace: 2, phase: 'RUNNING', run_active: true,
    task_time_estimate: {status: 'estimated', label: '~1h 1m left'}, workflow: {fraction: .4}};
const numberedCodex = [
    {...numberedBase, session_id: 'cx-one', kind: 'codex', codex_instance: 1, instance: '3'},
    {...numberedBase, session_id: 'cx-two', kind: 'codex', codex_instance: 2, instance: '8'},
    {...numberedBase, session_id: 'cx-unknown', kind: 'codex', instance: '6'},
    {...numberedBase, session_id: 'cx-one-done', kind: 'codex', codex_instance: 1, phase: 'DONE'},
];
const numberedVibe = [
    {...numberedBase, session_id: 'va-one', kind: 'vibe', instance: '1'},
    {...numberedBase, session_id: 'va-ten', kind: 'vibe', instance: '10'},
    {...numberedBase, session_id: 'va-invalid', kind: 'vibe', instance: 'none'},
    {...numberedBase, session_id: 'va-other', kind: 'vibe', instance: '2', project: 'different'},
];
const numberedPayload = {codexInstanceCount: 2, providers: [
    {kind: 'codex', agents: numberedCodex}, {kind: 'vibe', agents: numberedVibe},
    {kind: 'claude', agents: [{...numberedBase, session_id: 'cc-only', instance: '1'}]},
]};
const quotaNow = Math.floor(Date.now() / 1000);
const quotaAccounts = [
    {id: 1, windows: [{label: '5h', used_percent: 100, resets_at: quotaNow + 600}]},
    {id: 2, windows: [{label: 'week', used_percent: 99, resets_at: quotaNow + 600}]},
];
let quotaContents = () => JSON.stringify({clis: {codex: {instances: quotaAccounts}}});
const quotaFile = {load_contents: () => [true, new TextEncoder().encode(quotaContents())]};
const numberedButton = Object.assign({
    _iconsDir: iconsDir, _box: new Actor(), _codexLimitFile: quotaFile,
}, owners.updater);
numberedButton._updatePanelProgress(numberedPayload, false);
for (const [key, expected] of [
    ['cx-one', '1'], ['cx-one-done', '1'], ['cx-two', '2'], ['cx-unknown', ''],
    ['va-one', '1'], ['va-ten', '10'], ['va-invalid', ''], ['va-other', ''], ['cc-only', ''],
]) {
    const bar = numberedButton._panelProgressBars.get(key);
    const instance = bar._amsInstanceLabel;
    const estimate = bar._amsEstimateLabel;
    const footer = estimate.get_parent();
    check(footer !== bar.get_parent() && footer.y_align === Clutter.ActorAlign.END &&
        footer.x_align === Clutter.ActorAlign.FILL, 'full-width footer sits below the bar');
    check(instance.text === expected && instance.visible === Boolean(expected),
        `published instance number for ${key} is ${expected}`);
    check(footer.children[0] === instance && instance.x_align === Clutter.ActorAlign.START &&
        footer.children[1] === estimate && estimate.x_align === Clutter.ActorAlign.END,
        'instance stays on the left and ETA is anchored to the right within the same footer');
}
const badgeFor = key => numberedButton._panelProgressBars.get(key)._amsInstanceLabel;
check(badgeFor('cx-one').style === 'color: #ef4444;' &&
    badgeFor('cx-one-done').style === 'color: #ef4444;',
    'every badge for an exhausted Codex account is red regardless of task phase');
check(!badgeFor('cx-two').style && !badgeFor('va-one').style,
    'available Codex and unrelated project-local numbers keep their usual color');
quotaAccounts[0].windows[0].resets_at = quotaNow - 1;
quotaAccounts[1].windows[0].used_percent = 100;
numberedButton._updatePanelProgress(numberedPayload, false);
check(!badgeFor('cx-one').style && badgeFor('cx-two').style === 'color: #ef4444;',
    'rollover restores the default color and a newly exhausted weekly limit turns red');
quotaAccounts[0].windows = [{label: 'luna_reserve', used_percent: 100, resets_at: quotaNow + 600}];
quotaAccounts[1].windows = [];
quotaAccounts[1].credits = {blocked: true};
numberedButton._updatePanelProgress(numberedPayload, false);
check(!badgeFor('cx-one').style && badgeFor('cx-two').style === 'color: #ef4444;',
    'a supplementary reserve alone leaves the badge normal, and a credit block colors its account red');
quotaContents = () => '{broken';
numberedButton._updatePanelProgress(numberedPayload, false);
check(!badgeFor('cx-one').style && !badgeFor('cx-two').style,
    'an unreadable quota snapshot leaves account colors at their default');
const numberedBar = numberedButton._panelProgressBars.get('cx-two');
const numberedLabel = numberedBar._amsInstanceLabel;
numberedCodex[1].codex_instance = 1;
numberedCodex[1].run_active = false;
numberedButton._updatePanelProgress(numberedPayload, true);
check(numberedButton._panelProgressBars.get('cx-two') === numberedBar && numberedBar._amsInstanceLabel === numberedLabel &&
    numberedLabel.text === '1' && numberedLabel.opacity === 128 && !numberedBar._amsEstimateLabel.visible,
    'retargeting updates the number in place and a missing ETA leaves it visible');
numberedButton._updatePanelProgress({codexInstanceCount: 1, providers: [
    {kind: 'codex', agents: [numberedCodex[1]]},
]}, false);
check(!numberedButton._panelProgressBars.get('cx-two')._amsInstanceLabel.visible,
    'a single Codex login and session needs no instance label');
numberedButton._updatePanelProgress({codexInstanceCount: 1, providers: [
    {kind: 'codex', agents: [numberedCodex[0], numberedCodex[1]]},
]}, false);
check(numberedButton._panelProgressBars.get('cx-one')._amsInstanceLabel.text === '3' &&
    numberedButton._panelProgressBars.get('cx-two')._amsInstanceLabel.text === '8',
    'multiple tracked sessions sharing one login show project-local instance numbers');
numberedButton._updatePanelProgress(null, false);


const retainedEstimateCell = estimateCells[0];
const retainedEstimateBar = retainedEstimateCell._amsBar;
const retainedEstimateLabel = retainedEstimateCell._amsBar._amsEstimateLabel;
estimateAgents[0].task_time_estimate.label = '~1m left';
button._updatePanelProgress(estimatePayload, false);
check(estimateRow.children[0] === retainedEstimateCell && retainedEstimateCell._amsBar === retainedEstimateBar &&
    retainedEstimateCell._amsBar._amsEstimateLabel === retainedEstimateLabel && retainedEstimateLabel.text === '~1m',
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
    check(estimateCells[1]._amsBar._amsEstimateLabel.text === '~1h1m' && estimateCells[1]._amsBar._amsEstimateLabel.visible,
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
            check(fill.color[3] === (fraction === 1 ? 1 : fraction > 0 ? .72 : .22), 'zero-progress draft invents no opaque fill');
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
check(localEstimateCell._amsBar._amsEstimateLabel.text === '~3m', 'removing aggregate keeps the running agent estimate');
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

// Runtime is an HH:MM clock above active runs; confirmed finishes show their age.
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
check(runtimeCells.every(cell => cell.children.length === 4), 'each cell contains a logo, bar, lower forecast and upper runtime');
const liveRuntime = runtimeCells[0]._amsBar._amsRuntimeLabel;
const doneRuntime = runtimeCells[1]._amsBar._amsRuntimeLabel;
check(liveRuntime.text === '01:01' && doneRuntime.text === '1h ago' && doneRuntime.visible, 'live timer formats hours:minutes and finish age comes from the confirmed timestamp');
for (const cell of runtimeCells) {
    const runtime = cell._amsBar._amsRuntimeLabel;
    check(runtime.style_class.split(' ').includes('ams-panel-agent-runtime') && runtime.x_align === Clutter.ActorAlign.CENTER &&
        runtime.y_align === Clutter.ActorAlign.START && runtime.x_expand === true && runtime.y_expand === true && !runtime.reactive,
        'runtime sits at the top centered above the retained bar');
}
check(runtimeCells[0]._amsBar._amsEstimateLabel.text === '~3m' && !runtimeCells[1]._amsBar._amsEstimateLabel.visible,
    'running forecast remains below while completed runtime has no remaining estimate');
button._applyPanelProgressPulse(runtimeReference + 60 * 1000);
check(liveRuntime.text === '01:02' && doneRuntime.text === '1h ago' && doneRuntime.visible, 'live minute ticks retain the completion age');
check(liveRuntime.opacity === 255 && doneRuntime.opacity === 255, 'runtime labels do not pulse with progress fill');
for (const [seconds, expected] of [[0, '00:00'], [59, '00:00'], [60, '00:01'], [3599, '00:59'],
    [3600, '01:00'], [3660, '01:01'], [90000, '25:00'], [360000, '100:00']]) {
    runtimeAgents[0].run_started_at = new Date(runtimeReference - seconds * 1000).toISOString();
    runtimeAgents[1].task_time_estimate.observed_active_seconds = seconds;
    button._updatePanelProgress(runtimePayload, false);
    check(liveRuntime.text === expected && doneRuntime.text === '1h ago' && doneRuntime.visible, 'active HH:MM truncates partial minutes independently of the completion age');
}
runtimeAgents[0].run_started_at = new Date(runtimeReference + 60000).toISOString();
button._updatePanelProgress(runtimePayload, false);
check(liveRuntime.text === '00:00', 'future live timestamp never produces negative runtime');
for (const value of [null, -1, NaN, Infinity, '120']) {
    runtimeAgents[0].run_started_at = 'invalid';
    runtimeAgents[0].task_time_estimate.observed_active_seconds = value;
    runtimeAgents[1].task_time_estimate.observed_active_seconds = value;
    button._updatePanelProgress(runtimePayload, false);
    check(liveRuntime.text === '' && !liveRuntime.visible && doneRuntime.text === '1h ago' && doneRuntime.visible,
        'invalid active duration stays blank and does not affect the completion age');
}
for (const phase of ['DRAFT', 'STOPPED', 'ATTN', 'RATE', 'INTERRUPTED']) {
    runtimeAgents[1].phase = phase;
    runtimeAgents[1].task_time_estimate.observed_active_seconds = 300;
    button._updatePanelProgress(runtimePayload, false);
    check(doneRuntime.text === '' && !doneRuntime.visible, 'nonrunning unfinished states do not invent a completed runtime');
}
Object.assign(runtimeAgents[1], {phase: 'DONE', run_started_at: '2026-10-13T07:45:00Z',
    ready_completed_at: '2026-10-13T09:00:00Z', unread_completion: true, task_time_estimate: null});
button._updatePanelProgress(runtimePayload, false);
check(doneRuntime.text === '1h ago' && doneRuntime.visible, 'confirmed completion ignores the previous running clock');
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
check(runtimeRow.children[0] === runtimeCells[0] && liveRuntime.text === '' && !liveRuntime.visible &&
    !runtimeCells[0]._amsBar._amsEstimateLabel.visible, 'finishing run immediately clears runtime and remaining estimate on same actors');
button._applyPanelProgressPulse(runtimeReference + 3600000);
check(liveRuntime.text === '' && !liveRuntime.visible, 'completed runtime stays hidden on later animation ticks');
// Rebuild with the same completed payload, as after an extension reload.
button._updatePanelProgress(null, false);
button._updatePanelProgress(runtimePayload, false);
check(button._panelProgress.children[0].children[1].children[0]._amsBar._amsRuntimeLabel.text === '' &&
    !button._panelProgress.children[0].children[1].children[0]._amsBar._amsRuntimeLabel.visible,
    'completed timer remains hidden after rebuilding from its published measurement');
snapshot = runtimePayload;
button.menu.isOpen = true;
button._refresh();
check(button._panelProgress.children[0].children[1].children[0]._amsBar._amsRuntimeLabel.text === '' &&
    !button._panelProgress.children[0].children[1].children[0]._amsBar._amsRuntimeLabel.visible, 'open-menu refresh keeps completed timer hidden');
button.menu.isOpen = false;
button._updatePanelProgress(null, false);
Date.now = savedDateNow;
print('PASS: active-only HH:MM runtime, minute ticks, completion clearing and hidden completed timers after reload.');
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

// Runtime text grows exactly 1.5x while leaving room above the centered fill.
const runtimeStyle = stylesheet.match(/\.ams-panel-agent-runtime\s*\{([^}]+)\}/)?.[1] || '';
check(/font-size:\s*13\.5px;/.test(runtimeStyle), 'runtime timer grows from 9px to 13.5px');
check(/margin-top:\s*1px;/.test(runtimeStyle), 'larger runtime text retains clearance above the centered progress bar');
print('PASS: runtime timer is 1.5x larger with clearance above the bar.');

// A visible runtime carries the goal cue, and changes without rebuilding its cell.
const savedColorDateNow = Date.now;
const timerColorReference = Date.parse('2026-10-13T10:00:00Z');
Date.now = () => timerColorReference;
const timerColorAgents = [
    {session_id: 'timer-regular', workspace: 2, monitor: 0, window_display_id: 1, phase: 'RUNNING', run_active: true,
        goal_active: false, run_started_at: new Date(timerColorReference - 600000).toISOString(),
        workflow: {fraction: .5}, task_time_estimate: {status: 'estimated', label: '~3m left'}},
    {session_id: 'timer-goal', workspace: 2, monitor: 0, window_display_id: 2, phase: 'RUNNING', run_active: true,
        goal_active: true, focused: true, run_started_at: new Date(timerColorReference - 3660000).toISOString(),
        workflow: {fraction: .5}, task_time_estimate: {status: 'estimated', label: '~4m left'}},
];
const timerColorPayload = {providers: [{kind: 'codex', agents: timerColorAgents}]};
button._updatePanelProgress(timerColorPayload, false);
const regularTimerBar = button._panelProgressBars.get('timer-regular');
const goalTimerBar = button._panelProgressBars.get('timer-goal');
const regularTimer = regularTimerBar._amsRuntimeLabel;
const goalTimer = goalTimerBar._amsRuntimeLabel;
check(regularTimer.visible && regularTimer.text === '00:10' && regularTimer.style === 'color: #e6c200;',
    'regular run runtime is yellow');
check(goalTimer.visible && goalTimer.text === '01:01' && goalTimer.style === 'color: #a855f7;',
    'goal runtime is purple');
check(regularTimerBar.strokes.length === 0 && goalTimerBar.strokes.length === 1 && goalTimerBar.strokes[0].width === 1,
    'neither regular nor focused goal bar has a purple partition');
const regularTimerCell = regularTimerBar.get_parent();
timerColorAgents[0].goal_active = true;
timerColorAgents[1].goal_active = false;
button._updatePanelProgress(timerColorPayload, false);
check(regularTimerBar.get_parent() === regularTimerCell && regularTimer.style === 'color: #a855f7;' &&
    goalTimer.style === 'color: #e6c200;', 'switching goal state recolors retained timer actors in both directions');
button._applyPanelProgressPulse(timerColorReference + 60000);
check(regularTimer.text === '00:11' && regularTimer.style === 'color: #a855f7;' && regularTimer.opacity === 255 &&
    goalTimer.style === 'color: #e6c200;' && goalTimer.opacity === 255, 'minute ticks and bar pulses preserve steady timer colors');
button._updatePanelProgress({...timerColorPayload, updatedAt: new Date(timerColorReference).toISOString()}, true);
check(regularTimer.style === 'color: #a855f7;' && regularTimer.opacity === 128 &&
    goalTimer.style === 'color: #e6c200;' && goalTimer.opacity === 128, 'stale timers retain identity color and normal dimming');
Object.assign(timerColorAgents[0], {phase: 'DONE', run_active: false});
button._updatePanelProgress(timerColorPayload, false);
check(!regularTimer.visible && regularTimer.text === '', 'completed goal never leaves a colored timer visible');
check(regularTimerBar.strokes.every(stroke => stroke.width === 1) && goalTimerBar.strokes.every(stroke => stroke.width === 1),
    'state transitions never restore a purple stripe');
button._updatePanelProgress(null, false);
Date.now = savedColorDateNow;
print('PASS: yellow regular runtime, purple goal runtime, retained state transitions and no goal stripe.');

// A rate-limited launch countdown uses the upper timer and leaves the lower estimate blank.
const savedResetNow = Date.now;
const resetReference = Date.parse('2026-10-14T12:00:00Z');
Date.now = () => resetReference;
const resetAgents = [
    {session_id: 'reset-rate', workspace: 2, monitor: 0, window_display_id: 1, phase: 'RATE',
        run_active: false, goal_active: true, queued_reset_at: new Date(resetReference + (2 * 3600 + 45 * 60 + 50) * 1000).toISOString(),
        task_time_estimate: {status: 'estimated', label: '~9m left'}, workflow: {fraction: 0.4}},
    {session_id: 'reset-running', workspace: 2, monitor: 0, window_display_id: 2, phase: 'RUNNING',
        run_active: true, goal_active: false, run_started_at: new Date(resetReference - 60 * 1000).toISOString(),
        task_time_estimate: {status: 'estimated', label: '~5m left'}, workflow: {fraction: 0.2}},
];
const resetPayload = {providers: [{kind: 'codex', agents: resetAgents}]};
button._updatePanelProgress(resetPayload, false);
const resetCell = button._panelProgressBars.get('reset-rate').get_parent();
const runningCell = button._panelProgressBars.get('reset-running').get_parent();
const resetUpper = resetCell._amsBar._amsRuntimeLabel;
const resetLower = resetCell._amsBar._amsEstimateLabel;
const runningUpper = runningCell._amsBar._amsRuntimeLabel;
const runningLower = runningCell._amsBar._amsEstimateLabel;
check(resetUpper.style_class === 'ams-panel-agent-runtime' && resetUpper.y_align === Clutter.ActorAlign.START &&
    resetUpper.visible && resetUpper.text === '-02:45' && resetUpper.style === 'color: #b5d747;',
    'rate-limited launch countdown sits above the bar like the run timer');
check(resetLower.text === '' && !resetLower.visible, 'rate-limited bar does not show time left underneath');
check(runningUpper.text === '00:01' && runningUpper.style === 'color: #e6c200;' &&
    runningLower.text === '~5m' && runningLower.visible,
    'a neighboring run keeps its upper clock and lower remaining estimate');
for (const [seconds, expected] of [[1, '-00:01'], [59, '-00:01'], [60, '-00:01'], [61, '-00:01'],
    [3599, '-00:59'], [3600, '-01:00'], [3661, '-01:01'], [2 * 3600 + 47 * 60 + 55, '-02:47'],
    [100 * 3600, '-100:00']]) {
    resetAgents[0].queued_reset_at = new Date(resetReference + seconds * 1000).toISOString();
    button._updatePanelProgress(resetPayload, false);
    check(resetUpper.text === expected && resetUpper.visible && resetLower.text === '' && !resetLower.visible,
        `launch countdown ${seconds}s renders ${expected} without a lower estimate`);
    check(runningUpper.text === '00:01' && runningLower.text === '~5m',
        'countdown boundaries leave the neighboring run timers unchanged');
}
resetAgents[0].phase = 'INTERRUPTED';
resetAgents[0].queued_reset_at = new Date(resetReference + (2 * 3600 + 45 * 60 + 50) * 1000).toISOString();
button._updatePanelProgress(resetPayload, false);
check(resetUpper.text === '-02:45' && resetUpper.style === 'color: #ef4444;' && !resetLower.visible,
    'an interrupted wait keeps the red countdown and still hides time left');
for (const queued of ['', 'not-a-time', new Date(resetReference).toISOString(), new Date(resetReference - 1000).toISOString()]) {
    resetAgents[0].queued_reset_at = queued;
    resetAgents[0].phase = 'RATE';
    button._updatePanelProgress(resetPayload, false);
    check(resetUpper.text === '' && !resetUpper.visible && resetLower.text === '' && !resetLower.visible,
        'missing, invalid, or expired resets do not invent a countdown or a lower estimate');
    check(runningUpper.text === '00:01' && runningLower.text === '~5m' && runningLower.visible,
        'a rejected reset leaves the running neighbor timers in place');
}
resetAgents[0].queued_reset_at = new Date(resetReference + 125 * 1000).toISOString();
resetAgents[0].phase = 'WAITING';
button._updatePanelProgress(resetPayload, false);
check(resetUpper.text === '-00:02' && resetUpper.style === 'color: #b5d747;', 'two whole minutes remain above the bar');
button._applyPanelProgressPulse(resetReference + 60 * 1000);
check(resetUpper.text === '-00:01' && resetLower.text === '' && runningUpper.text === '00:02' && runningLower.text === '~5m',
    'the launch countdown ticks down a minute while the running estimate stays below its own bar');
button._updatePanelProgress({...resetPayload, updatedAt: new Date(resetReference).toISOString()}, true);
const staleResetText = resetUpper.text;
button._applyPanelProgressPulse(resetReference + 180 * 1000);
check(resetUpper.text === staleResetText && resetUpper.opacity === 128 && !resetLower.visible,
    'a stale launch countdown stays dim and does not keep ticking');
button._updatePanelProgress(resetPayload, false);
check(resetUpper.opacity === 255 && resetUpper.text === '-00:02', 'a fresh snapshot resumes the launch countdown');
Object.assign(resetAgents[0], {phase: 'RATE', run_active: false,
    workflow: {fraction: 1, phase_completed: 6, phase_total: 6},
    queued_reset_at: new Date(resetReference + 7200 * 1000).toISOString(),
    task_time_estimate: {status: 'estimated', label: '~9m left'}});
button._updatePanelProgress(resetPayload, false);
check(resetUpper.text === '' && !resetUpper.visible && resetLower.text === '' && !resetLower.visible,
    'finished work shows neither a launch countdown nor time left underneath');
const finishedBar = button._panelProgressBars.get('reset-rate');
finishedBar.queue_repaint();
const finishedRgb = finishedBar.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255));
check(JSON.stringify(finishedRgb) !== '[244,114,182]', 'a finished plan is not painted with the rate-limit color');
check(runningUpper.text === '00:01' && runningLower.text === '~5m',
    'marking one plan finished leaves the running neighbor timers alone');
button._updatePanelProgress(null, false);
Date.now = savedResetNow;
print('PASS: rate-limit launch countdown above the bar, with no time left underneath.');
// GNOME clears hover if an attached instance subtree is removed, even when reused.
button._renderProvider = provider => button._box.add_child(new Actor({
    style_class: 'ams-segment', text: String(provider.running || 0), reactive: false}));
button._renderPanelHarnessRates = () => button._box.add_child(new Actor({
    style_class: 'ams-rate', text: '64/s', reactive: false}));
button._renderMenu = payload => { button.lastMenuPayload = payload; };
snapshot = selectionPayload;
button.menu.isOpen = false;
button._refresh();
const steadyStrip = button._panelProgress;
const steadyCell = selectionCell('select-a');
const initialDetachments = steadyCell.stageDetachments || 0;
const initialRestackings = steadyStrip.restackings || 0;
const steadySummaries = button._box.get_children().filter(child => child !== steadyStrip);
for (let motion = 0; motion < 20; motion++) {
    steadyCell.hover = true;
    snapshot = {...selectionPayload, providers: [{kind: 'codex', running: motion % 3,
        agents: selectionAgents.map(agent => ({...agent, workflow: {fraction: motion / 20}}))}]};
    button._refresh();
    check(steadyCell.hover === true, 'public status refresh must not interrupt pointer hover');
    check(steadySummaries.every(child => !child.destroyed && child.get_parent() === button._box),
        'hover refresh must preserve surrounding summary actors and their allocations');
    check(button.lastMenuPayload === snapshot, 'hover keeps detailed-menu data current');
    check((steadyStrip.restackings || 0) === initialRestackings,
        'hovered instance strip must not be restacked during summary refresh');
    check((steadyCell.stageDetachments || 0) === initialDetachments && steadyStrip.get_parent() === button._box,
        'hovered instance strip stays continuously attached through refresh');
    check(selectionCell('select-a') === steadyCell && steadyCell.width === 52 && steadyCell.y_expand,
        'preserve the exact existing full-height highlight area and actor');
    check(button._box.children.at(-1) === steadyStrip && button._box.children.length === 3,
        'fresh provider/rate summaries precede the retained strip without accumulating old chips');
    check(button._panelProgressBars.get('select-a')._amsAgent.workflow.fraction === motion / 20,
        'retained hover never freezes live task updates');
    check(!selectionCell('select-b').hover, 'neighbor remains unhighlighted during pointer movement');
}
steadyCell.hover = false;
button._refresh();
check(steadySummaries.every(child => child.destroyed) && button._box.children[0].text === '1',
    'leaving the instance applies the latest deferred provider counts');
steadyCell.hover = true;
const steadySelections = selectedNotes.length;
steadyCell['button-press-event'](steadyCell, {get_button: () => 1});
check(selectedNotes.length === steadySelections + 1 && selectedNotes.at(-1) === 'note-a',
    'steady hover preserves exact instance click action');
for (const open of [true, false, true, false]) {
    button.menu.isOpen = open;
    button._refresh();
    check(steadyCell.hover && (steadyCell.stageDetachments || 0) === initialDetachments,
        'opening or dismissing details never detaches the hovered instance');
}
snapshot = {providers: []};
button.menu.isOpen = false;
button._refresh();
check(steadyCell.destroyed && button._panelProgressBars.size === 0 && !button._panelProgress.visible,
    'closing all terminals still destroys their click targets and hides the strip');
snapshot = selectionPayload;
button._refresh();
check(selectionCell('select-a') !== steadyCell && selectionCell('select-a').reactive,
    'freshly reopened terminals recover their selection and hover targets');
snapshot = null;
button._refresh();
check(button._panelProgressBars.size === 0 && !button._panelProgress.visible,
    'missing publication still clears obsolete instances');
print('PASS: continuous stage attachment during pointer/closed-menu refresh, unchanged highlight area, current updates, clicks and recovery.');

// Keep only the forecast duration; no tail after its minute/hour unit reaches the panel.
const compactForecastAgent = {session_id: 'compact-forecast', workspace: 5, monitor: 0,
    phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated', label: '~7m left'}};
const compactForecastPayload = {providers: [{kind: 'codex', agents: [compactForecastAgent]}]};
button._updatePanelProgress(compactForecastPayload, false);
const compactForecastBar = button._panelProgressBars.get('compact-forecast');
const compactForecastLabel = compactForecastBar._amsEstimateLabel;
for (const [label, expected] of [
    ['~7m left', '~7m'], ['~7m left (historical)', '~7m'], ['~7m remaining', '~7m'],
    ['~7mleft', '~7m'], ['~7m l', '~7m'], ['  ~7m left  ', '~7m'], ['~7m', '~7m'],
    ['~1h 30m left (historical)', '~1h30m'], ['~1h30m left', '~1h30m'],
    ['~1h 1m remaining', '~1h1m'], ['~1h left', '~1h'], ['7m extra text', '7m'],
    ['', ''], ['not available', ''],
]) {
    compactForecastAgent.task_time_estimate.label = label;
    button._updatePanelProgress(compactForecastPayload, false);
    check(compactForecastBar._amsEstimateLabel === compactForecastLabel && compactForecastLabel.text === expected &&
        compactForecastLabel.visible === Boolean(expected), 'only a compact duration appears on the retained forecast label');
}
compactForecastAgent.task_time_estimate.label = '~4m remaining (historical)';
button._updatePanelProgress(compactForecastPayload, false);
snapshot = compactForecastPayload;
button.menu.isOpen = true;
button._refresh();
check(compactForecastLabel.text === '~4m', 'open-menu refresh cannot reintroduce trailing forecast text');
compactForecastAgent.phase = 'DONE';
compactForecastAgent.run_active = false;
button._updatePanelProgress(compactForecastPayload, false);
check(!compactForecastLabel.visible && compactForecastLabel.text === '', 'completed instance still has no remaining estimate');
button.menu.isOpen = false;
button._updatePanelProgress(null, false);
const forecastStyle = stylesheet.match(/\.ams-panel-agent-estimate\s*\{([^}]+)\}/)?.[1] || '';
check(/font-size:\s*13\.5px;/.test(forecastStyle), 'remaining estimate grows from 9px to 13.5px');
const footerStyle = stylesheet.match(/\.ams-panel-task-footer\s*\{([^}]+)\}/)?.[1] || '';
check(/font-family:\s*monospace;/.test(forecastStyle) && /margin:\s*0 1px 1px;/.test(footerStyle),
    'larger compact forecast fits below its bar with consistent timer typography');
print('PASS: 1.5x lower forecast text contains only duration, across suffixes and retained refreshes.');

// A completed task shows its finish age and shares the exact ready-bar pulse.
const completionClockNow = Date.parse('2026-10-14T10:00:00Z');
const completionClockAgent = {session_id: 'completion-clock', phase: 'RUNNING', run_active: true,
    run_started_at: '2026-10-14T08:45:00Z', note_id: 'completion-note',
    workflow: {available: true, fraction: .5}};
const completionClockPayload = {providers: [{kind: 'codex', agents: [completionClockAgent]}]};
Date.now = () => completionClockNow;
button._updatePanelProgress(completionClockPayload, false);
const completionClockBar = button._panelProgressBars.get('completion-clock');
const completionClockLabel = completionClockBar._amsRuntimeLabel;
check(completionClockLabel.text === '01:15', 'running taskbar clock uses its published task start');
Object.assign(completionClockAgent, {phase: 'DONE', run_active: false, unread_completion: true, run_started_at: '',
    ready_completed_at: '2026-10-14T10:00:00Z'});
button._updatePanelProgress(completionClockPayload, false);
check(button._panelProgressBars.get('completion-clock') === completionClockBar &&
    completionClockLabel.text === '0s ago' && completionClockLabel.visible &&
    completionClockLabel.style_class.includes('ams-panel-agent-age'),
    'finish age replaces the running clock on the same taskbar actor');
for (const [offset, expected, age] of [[0, [34,197,94], '0s ago'], [750, [145,226,175], '0s ago'],
    [1500, [255,255,255], '1s ago'], [2250, [145,226,175], '2s ago'], [3000, [34,197,94], '3s ago'],
    [57000, [34,197,94], '57s ago'], [60000, [34,197,94], '1m ago'],
    [3600000, [34,197,94], '1h ago'], [86400000, [34,197,94], '1d ago']]) {
    button._applyPanelProgressPulse(completionClockNow + offset);
    check(completionClockLabel.text === age && completionClockLabel.visible,
        `completion age uses the confirmed finish at offset ${offset}`);
    check(completionClockLabel.style === `color: rgb(${expected.join(', ')});`,
        'timer flashes green to white on the same three-second wave as its bar');
    const fill = completionClockBar.draws.at(-1).color.slice(0, 3).map(channel => Math.round(channel * 255));
    check(fill.every((channel, index) => Math.abs(channel - expected[index]) <= 1),
        'timer and progress fill sample the same flash at every phase');
}
Date.now = () => completionClockNow + 1500;
button._updatePanelProgress(completionClockPayload, true);
check(completionClockLabel.style === 'color: rgb(34, 197, 94);' && completionClockLabel.opacity === 128,
    'stale completed timer stops flashing and dims with its bar');
button._applyPanelProgressPulse(completionClockNow + 2250);
check(completionClockLabel.style === 'color: rgb(34, 197, 94);', 'stale animation ticks cannot flash the timer');
for (const [finish, taskComplete] of [['', true], ['invalid', true],
    ['2026-10-14T10:01:00Z', true], ['2026-10-14T10:00:00Z', false]]) {
    completionClockAgent.ready_completed_at = finish;
    completionClockAgent.workflow.task_complete = taskComplete;
    button._updatePanelProgress(completionClockPayload, false);
    check(!completionClockLabel.visible && completionClockLabel.text === '',
        'unknown or future finishes and incomplete workflows have no age label');
}
button._updatePanelProgress(null, false);
Date.now = savedDateNow;
print('PASS: completed taskbar clock counts from the finish and flashes in sync with the ready progress bar.');

// Completion visibility follows the real unread/focus state, independently per session.
const applyCompletionState = new Function('print', `${declaration('unreadCompletionCount')}\nreturn unreadCompletionCount;`)(print);
const acknowledgementNow = Date.parse('2026-10-14T11:00:00Z');
Date.now = () => acknowledgementNow;
const acknowledgementStates = new Map();
const acknowledgementAgents = ['first', 'second'].map(session_id => ({session_id, workspace: 1,
    phase: 'RUNNING', run_active: true, focused: false, run_started_at: '2026-10-14T10:45:00Z'}));
const acknowledgementPayload = {providers: [{kind: 'codex', agents: acknowledgementAgents}]};
applyCompletionState(acknowledgementPayload, acknowledgementStates, acknowledgementNow);
button._updatePanelProgress(acknowledgementPayload, false);
for (const agent of acknowledgementAgents)
    Object.assign(agent, {phase: 'DONE', run_active: false, run_started_at: '',
        ready_completed_at: '2026-10-14T11:00:00Z'});
check(applyCompletionState(acknowledgementPayload, acknowledgementStates, acknowledgementNow) === 2,
    'two newly finished sessions independently await acknowledgement');
button._updatePanelProgress(acknowledgementPayload, false);
const firstAcknowledgedBar = button._panelProgressBars.get('first');
const secondAcknowledgedBar = button._panelProgressBars.get('second');
check(firstAcknowledgedBar._amsRuntimeLabel.text === '0s ago' && secondAcknowledgedBar._amsRuntimeLabel.text === '0s ago',
    'both completed sessions show their confirmed finish age');
acknowledgementAgents[0].focused = true;
check(applyCompletionState(acknowledgementPayload, acknowledgementStates, acknowledgementNow) === 1,
    'focusing a finished session acknowledges only that session');
button._updatePanelProgress(acknowledgementPayload, false);
check(firstAcknowledgedBar._amsRuntimeLabel.text === '0s ago' && firstAcknowledgedBar._amsRuntimeLabel.visible,
    'acknowledgement leaves the finish age on the same actor');
check(secondAcknowledgedBar._amsRuntimeLabel.visible && secondAcknowledgedBar._amsRuntimeLabel.text === '0s ago',
    'acknowledging one session preserves another unread timer');
button._applyPanelProgressPulse(acknowledgementNow + 1500);
check(firstAcknowledgedBar._amsRuntimeLabel.style === 'color: rgb(34, 197, 94);' && firstAcknowledgedBar.opacity === 255,
    'acknowledgement ends the pulse on the age label and its bar');
check(secondAcknowledgedBar._amsRuntimeLabel.style === 'color: rgb(255, 255, 255);',
    'the other unread session keeps flashing');
acknowledgementAgents[0].focused = false;
Date.now = () => acknowledgementNow + 10000;
applyCompletionState(acknowledgementPayload, acknowledgementStates, Date.now());
button._updatePanelProgress(acknowledgementPayload, false);
check(!acknowledgementAgents[0].unread_completion && firstAcknowledgedBar._amsRuntimeLabel.visible,
    'leaving a checked session preserves the age without marking it unread again');
// Reordering/rebuilding cells for a workspace move must not discard an unread clock.
acknowledgementAgents[1].workspace = 2;
button._updatePanelProgress(acknowledgementPayload, false);
check(button._panelProgressBars.get('second')._amsRuntimeLabel.text === '10s ago',
    'workspace regrouping preserves the completed session timer');
Object.assign(acknowledgementAgents[0], {phase: 'RUNNING', run_active: true,
    ready_completed_at: '', run_started_at: '2026-10-14T10:58:10Z'});
applyCompletionState(acknowledgementPayload, acknowledgementStates, Date.now());
button._updatePanelProgress(acknowledgementPayload, false);
check(button._panelProgressBars.get('first')._amsRuntimeLabel.text === '00:02',
    'a new task restores the timer using its own start');
Object.assign(acknowledgementAgents[0], {phase: 'DONE', run_active: false, run_started_at: '',
    ready_completed_at: '2026-10-14T11:00:11Z'});
Date.now = () => acknowledgementNow + 11000;
applyCompletionState(acknowledgementPayload, acknowledgementStates, Date.now());
button._updatePanelProgress(acknowledgementPayload, false);
check(button._panelProgressBars.get('first')._amsRuntimeLabel.text === '0s ago',
    'a later completion bases the age on the new finish');
// Both read and unread completion ages can be rebuilt from the payload alone.
button._updatePanelProgress(null, false);
Object.assign(acknowledgementAgents[0], {focused: true, run_started_at: '2026-10-14T10:58:10Z'});
Object.assign(acknowledgementAgents[1], {task_time_estimate: {status: 'complete', observed_active_seconds: 900}});
const restoredAcknowledgements = new Map(JSON.parse(JSON.stringify([...acknowledgementStates])));
applyCompletionState(acknowledgementPayload, restoredAcknowledgements, acknowledgementNow + 11000);
button._updatePanelProgress(acknowledgementPayload, false);
check(button._panelProgressBars.get('first')._amsRuntimeLabel.text === '0s ago' &&
    !acknowledgementAgents[0].unread_completion, 'restored acknowledged session has an age without a pulse');
check(button._panelProgressBars.get('second')._amsRuntimeLabel.text === '11s ago',
    'restored unread age comes from the published finish despite a different measured runtime');
acknowledgementAgents[1].focused = true;
applyCompletionState(acknowledgementPayload, restoredAcknowledgements, Date.now());
button._updatePanelProgress(acknowledgementPayload, false);
Date.now = () => acknowledgementNow + 12000;
check(timers.size === 1, 'acknowledged completions share one clock update source');
for (const tick of timers.values())
    tick();
check(button._panelProgressBars.get('first')._amsRuntimeLabel.text === '1s ago' &&
    button._panelProgressBars.get('second')._amsRuntimeLabel.text === '12s ago',
    'acknowledged ages advance in place even when every task is idle');
for (const agent of acknowledgementAgents)
    Object.assign(agent, {phase: 'STOPPED', run_active: false, unread_completion: false});
button._updatePanelProgress(acknowledgementPayload, false);
check([...button._panelProgressBars.values()].every(bar => !bar._amsRuntimeLabel.visible) && timers.size === 0,
    'stopped sessions expose neither runtime labels nor completion animation');
button._updatePanelProgress(null, false);
Date.now = savedDateNow;
print('PASS: completion ages persist through acknowledgement, workspace changes and reload.');

// Harbor uses the actual existing workspace grouping and cell/fill owners.
button._updatePanelProgress(null, false);
const harborJob = {id: 'shared-run', name: 'shared-run', status: 'running', completed: 0,
    total: 20, owner_pids: [42], estimate: {remaining_seconds: 120}};
const harborWindows = [{pid: 42, workspace: 2, monitor: 0, focused: false}];
button._harborEntries = Harbor.harborPanelEntries([harborJob], harborWindows);
const sharedAgents = [{session_id: 'same-space', workspace: 2, monitor: 0, phase: 'RUNNING',
    run_active: true, workflow: {available: true, fraction: 0}},
    {session_id: 'other-space', workspace: 5, monitor: 0, phase: 'RUNNING', run_active: true,
    workflow: {available: true, fraction: 0.5}}];
const sharedPayload = {providers: [{kind: 'codex', label: 'Codex', agents: sharedAgents}]};
button._updatePanelProgress(sharedPayload, false);
let sharedGroups = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(sharedGroups.length === 2, 'Harbor joins existing workspace groups without an extra H W2 group');
const w2 = sharedGroups.find(group => group._amsWorkspace === 2);
check(w2.children[0].text === 'W2' && visibleBars({children: [w2]}).length === 2,
    'workspace 2 has one heading with both the agent and Harbor bar');
const sharedBars = visibleBars(button._panelProgress);
const harborBar = sharedBars.find(bar => bar._amsAgent.harbor);
const normalBar = sharedBars.find(bar => bar._amsAgent.session_id === 'same-space');
check(harborBar && normalBar, 'the common panel renderer creates both bar types');
check(harborBar.get_parent().width === normalBar.get_parent().width && harborBar.width === normalBar.width && harborBar.height === normalBar.height,
    'Harbor cell and fill geometry exactly match existing agent bars');
check(harborBar._amsRuntimeLabel.style_class === normalBar._amsRuntimeLabel.style_class &&
    harborBar._amsEstimateLabel.style_class === normalBar._amsEstimateLabel.style_class,
    'Harbor upper count and lower ETA use the existing typography');
check(harborBar._amsRuntimeLabel.text === '0/20' && harborBar._amsRuntimeLabel.style.includes('#f472b6'),
    'only the Harbor upper count uses the requested pink');
check(harborBar._amsEstimateLabel.text === '~2m', 'the normal lower ETA remains visible');
harborBar.queue_repaint(); normalBar.queue_repaint();
check(JSON.stringify(harborBar.draws) === JSON.stringify(normalBar.draws),
    'zero progress paints exactly like the normal agent bar, not pink');
button._harborEntries = Harbor.harborPanelEntries([{...harborJob, completed: 10}], harborWindows);
button._updatePanelProgress(sharedPayload, true);
check(button._panelProgressBars.get('harbor:shared-run') === harborBar && harborBar._amsRuntimeLabel.text === '10/20',
    'count updates retain the same shared bar actor');
check(!harborBar._amsStale && normalBar._amsStale, 'fresh independent Harbor progress survives stale ACC metadata');
button._applyPanelProgressPulse(Date.now() + 1000);
check(harborBar._amsRuntimeLabel.text === '10/20' && harborBar._amsRuntimeLabel.style.includes('#f472b6'),
    'shared clock animation must not overwrite Harbor counts or their color');
button._harborEntries = Harbor.harborPanelEntries([harborJob], [{...harborWindows[0], workspace: 5}]);
button._updatePanelProgress(sharedPayload, false);
sharedGroups = button._panelProgress.children.filter(child => child.style_class === 'ams-panel-workspace-group');
check(visibleBars({children: [sharedGroups.find(group => group._amsWorkspace === 5)]}).length === 2 &&
    visibleBars({children: [sharedGroups.find(group => group._amsWorkspace === 2)]}).length === 1,
    'moving the Harbor terminal moves only its bar to the actual workspace group');
button._updatePanelProgress(null, false);
check(visibleBars(button._panelProgress).length === 1 && visibleBars(button._panelProgress)[0]._amsRuntimeLabel.text === '0/20',
    'independent Harbor bars remain visible with no ACC payload');
button._harborEntries = [];
button._updatePanelProgress(null, false);
check(visibleBars(button._panelProgress).length === 0, 'ended Harbor membership leaves no separate panel strip');
print('PASS: Harbor shares workspace headings, normal cell/fill typography and geometry; only its count is pink.');
