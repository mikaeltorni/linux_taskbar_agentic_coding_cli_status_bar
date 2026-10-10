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
    add_child = child => this.children.push(child);
    destroy_all_children = () => { this.children = []; };
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
    });
}

const St = {BoxLayout: Actor, DrawingArea: Actor};
const Clutter = {ActorAlign: {CENTER: 0}};
const snippets = ['taskProgressValue', 'panelTaskProgressValue', 'panelProgressColor', 'panelProgressUnderway', 'panelProgressOpacity', 'paintPanelTaskProgress', 'buildPanelTaskProgress']
    .map(name => declaration(name));
const timers = new Map();
let nextTimer = 1;
const GLib = {PRIORITY_DEFAULT: 0, SOURCE_CONTINUE: true,
    timeout_add: (_priority, _delay, callback) => { const id = nextTimer++; timers.set(id, callback); return id; },
    source_remove: id => timers.delete(id),
};
const makeOwners = new Function('St', 'Clutter', 'GLib', 'print', `${snippets.join('\n')}
    class Destroyer extends St.BoxLayout { ${declaration('destroy', true)} }
    return {panelTaskProgressValue, panelProgressUnderway, panelProgressOpacity, paintPanelTaskProgress, buildPanelTaskProgress,
        Destroyer,
        updater: {${['_updatePanelProgress', '_syncPanelPulseTimer', '_applyPanelProgressPulse'].map(name => declaration(name, true)).join(',')}}};`);
const owners = makeOwners(St, Clutter, GLib, print);
const check = (condition, message) => { if (!condition) throw Error(message); };
const a = {session_id: 'a', phase: 'RUNNING', run_active: true, workflow: {available: true, fraction: 0.25}};
const b = {session_id: 'b', phase: 'STOPPED', workflow: {available: true, fraction: 0.6}};
const payload = {providers: [{kind: 'codex', agents: [a]}, {kind: 'claude', agents: [b]}]};
const button = Object.assign({_box: new Actor()}, owners.updater);
button._updatePanelProgress(payload, false);
let bars = button._panelProgress.children;
check(bars.length === 2, 'one bar for every open agent');
check(bars.every(bar => bar.width === 50 && bar.height === 8), '50px compact bars');
check(bars[0].draws.at(-1).rect[2] === 12.5, 'actual 25% fill');
check(bars[1].draws.at(-1).rect[2] === 30, 'actual 60% fill');
const retained = bars[0];
a.workflow.fraction = 0.8;
button._updatePanelProgress(payload, false);
bars = button._panelProgress.children;
check(bars[0] === retained, 'reuse actor during progress updates');
check(bars[0].draws.at(-1).rect[2] === 40, 'progress refresh to 80%');
const cases = [
    [{phase: 'DONE', ready_completed_at: '2026-10-10T10:00:00Z'}, 1],
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
