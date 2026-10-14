// Exercise production helpers without a desktop or user state.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '')
    .replace('export default class', 'class');
const check = (condition, message) => { if (!condition) throw Error(message); };
const traces = [];
const errors = [];
class Actor {
    stage = null;
    width = 200;
    height = 24;
    handlers = new Map();
    get_stage = () => this.stage;
    get_width = () => { check(this.stage !== null, 'detached geometry lookup causes GNOME theme warnings'); return this.width; };
    get_height = () => { check(this.stage !== null, 'detached geometry lookup causes GNOME theme warnings'); return this.height; };
    add_child = () => {};
    set_position = () => {};
    set_size = (width, height) => { this.width = width; this.height = height; };
    set_style = () => {};
    add_style_class_name = () => {};
    remove_style_class_name = () => {};
    connect = (name, callback) => this.handlers.set(name, callback);
}
const api = new Function('St', 'Clutter', 'GObject', 'PanelMenu', 'Extension', 'print', 'logError',
    `${source}\nreturn {displayedPhase, agentPhaseBadgeText, agentPhaseBadgeColor, readPayload, buildTaskProgressLayer, AccModelStatusButton};`)(
    {Widget: Actor}, {BinLayout: class {}, FixedLayout: class {}},
    {registerClass: value => value}, {Button: class {}}, class {},
    (...values) => traces.push(values.map(String).join(' ')),
    (...values) => errors.push(values));
let serializations = 0;
const agent = {phase: 'RUNNING', run_active: true, run_started_at: '2026-10-14T10:00:00Z',
    goal_active: true, workflow: {phase_completed: 2, phase_total: 4},
    toJSON: () => { serializations++; return {phase: 'RUNNING'}; }};
for (let i = 0; i < 100; i++) {
    check(api.displayedPhase(agent) === 'RUNNING', 'phase behavior is retained');
    check(api.agentPhaseBadgeText(agent, Date.parse('2026-10-14T10:01:30Z')) === '[2/4] 1:30',
        'workflow and runtime clock are retained');
    check(api.agentPhaseBadgeColor(agent, Date.parse('2026-10-14T10:01:30Z')) === '#a855f7',
        'goal color is retained');
}
check(traces.length === 0, `routine status updates emitted ${traces.length} trace messages`);
check(serializations === 0, `diagnostics serialized agent payloads ${serializations} times`);
const button = new api.AccModelStatusButton();
let actorDescriptions = 0;
button.toString = () => { actorDescriptions++; return 'off-stage actor'; };
button._taskHover = null;
button._hideTaskHover();
check(actorDescriptions === 0 && traces.length === 0, 'cleanup must not stringify off-stage actors');
check(api.readPayload({load_contents: () => [true, new TextEncoder().encode('{bad json')]}) === null,
    'invalid payload retains its safe fallback');
check(errors.length === 1 && errors[0][1] === 'acc-model-status: unparsable payload',
    'real payload errors remain visible');
check(traces.length === 0, 'error handling adds no diagnostic trace spam');
print('PASS: 300 status calls retain clocks/colors with zero traces, payload serialization or actor descriptions.');

const item = new Actor();
item.actor = item;
const widgets = api.buildTaskProgressLayer(item, new Actor(), new Actor(),
    {phase: 'RUNNING', workflow: {fraction: 0.5}});
widgets.track.stage = {};
widgets.track.handlers.get('notify::allocation')();
check(widgets.fill.width === 100 && widgets.fill.height === 24, 'attached tracks size the progress fill');
widgets.track.stage = null;
widgets.track.handlers.get('notify::allocation')();
check(widgets.fill.width === 100, 'detachment does not query theme or corrupt the retained fill');
print('PASS: detached construction and teardown skip geometry; attached allocation retains progress.');
