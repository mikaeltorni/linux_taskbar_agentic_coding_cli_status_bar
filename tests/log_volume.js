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
check(traces.some(value => value.startsWith('agent=')), 'authored badge functions print their parameter contracts');
const statusTraceCount = traces.length;
check(serializations === 0, `diagnostics serialized agent payloads ${serializations} times`);
const button = new api.AccModelStatusButton();
let actorDescriptions = 0;
button.toString = () => { actorDescriptions++; return 'off-stage actor'; };
button._taskHover = null;
button._hideTaskHover();
check(actorDescriptions === 0 && traces.length === statusTraceCount, 'cleanup must not stringify off-stage actors');
check(api.readPayload({load_contents: () => [true, new TextEncoder().encode('{bad json')]}) === null,
    'invalid payload retains its safe fallback');
check(errors.length === 1 && errors[0][1] === 'acc-model-status: unparsable payload',
    'real payload errors remain visible');
check(traces.length === statusTraceCount, 'error handling adds no diagnostic trace spam');
print('PASS: 300 status calls retain clocks/colors with parameter/return prints and zero payload serialization or actor descriptions.');

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

const [, rateBytes] = Gio.File.new_for_path(`${root}/agentic-cli-status@local/extension.js`).load_contents(null);
const rateSource = new TextDecoder().decode(rateBytes).replace(/^import .*;\n/gm, '')
    .replace('export default class', 'class');
const rates = new Function('GObject', 'PanelMenu', 'Extension', 'print',
    `${rateSource}\nreturn {codexBarWithForecastMarkup, codexResetForecastMenuEntries};`)(
    {registerClass: value => value}, {Button: class {}}, class {},
    (...values) => traces.push(values.map(String).join(' ')));
const forecastTraceCount = traces.length;
const forecast = {enabled: true, consensus_percent_24h: 28, consensus_percent_48h: 49,
    sites: [{name: 'Example', predictions: [{label: '24h', percent: 28}]}],
    toJSON: () => { serializations++; return {enabled: true}; }};
for (let i = 0; i < 100; i++) {
    const markup = rates.codexBarWithForecastMarkup(forecast, 'CX1', null, 0);
    check(markup.includes('28%') && markup.includes('49%'), 'forecast percentages are retained');
    const entries = rates.codexResetForecastMenuEntries(forecast);
    check(entries[1].text === 'Example — 24h: 28%', 'forecast menu entries are retained');
}
check(traces.length === forecastTraceCount && serializations === 0, 'forecast rendering must not dump snapshots into syslog');
print('PASS: 200 forecast calls retain output with zero traces or diagnostic serialization.');
