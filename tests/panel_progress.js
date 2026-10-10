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
    destroy = () => { this.destroyed = true; };
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
const snippets = ['taskProgressValue', 'panelTaskProgressValue', 'paintPanelTaskProgress', 'buildPanelTaskProgress']
    .map(name => declaration(name));
const makeOwners = new Function('St', 'Clutter', 'print', `${snippets.join('\n')}
    return {panelTaskProgressValue, paintPanelTaskProgress, buildPanelTaskProgress,
        updater: {${declaration('_updatePanelProgress', true)}}};`);
const owners = makeOwners(St, Clutter, print);
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
