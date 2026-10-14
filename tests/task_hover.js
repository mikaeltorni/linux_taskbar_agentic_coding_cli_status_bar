// Run the real hover owner against isolated Shell actors, without user state.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '')
    .replace('export default class', 'class');
const check = (condition, message) => { if (!condition) throw Error(message); };
const popups = [];
class Pointer {
    /** Construct a simulated anchored Shell popup.
     * Parameters: this - popup; side - arrow direction; options - actor options.
     * Returns: undefined.
     */
    constructor(side, options) {
        print(`this=${this} side=${side} options=${JSON.stringify(options)}`);
        this.side = side;
        this.visible = false;
        this.bin = {set_child: child => { this.child = child; }};
        popups.push(this);
        print(undefined);
    }
    setPosition = (actor, alignment) => { this.anchor = actor; this.alignment = alignment; };
    open = () => { this.visible = true; };
    destroy = () => { this.destroyed = true; this.visible = false; };
    add_style_class_name = () => {};
}
const Button = new Function('GObject', 'PanelMenu', 'Extension', 'BoxPointer', 'St', 'Main', 'print',
    `${source}\nreturn AccModelStatusButton;`)(
    {registerClass: value => value}, {Button: class {}}, class {},
    {BoxPointer: Pointer, PopupAnimation: {NONE: 0}}, {Side: {BOTTOM: 3}},
    {layoutManager: {addChrome: actor => { actor.chrome = true; }}}, print);
const button = new Button();
button.menu = {isOpen: false};
button._agentMenuItem = (agent, provider, count, track) => ({agent, provider, count, track, reactive: true, add_style_class_name: () => {}});
const cell = {hover: true};
const agent = {title: 'Monitor task placement', project_tag: 'ACC', branch: 'master', phase: 'DONE', note_id: '123'};
const provider = {kind: 'codex'};
button._syncTaskHover(cell, agent, provider, 3);
check(popups.length === 1 && popups[0].visible, 'hover opens a separate box');
check(popups[0].anchor === cell && popups[0].alignment === 0.5, 'box anchors to the task cell, independent of cursor');
check(popups[0].side === 3 && popups[0].chrome, 'bottom arrow puts box above taskbar using Shell chrome');
check(popups[0].child.agent === agent && popups[0].child.provider === provider, 'hover uses the same colored task details');
check(popups[0].child.track === false && popups[0].child.reactive === false, 'preview does not register interactive menu rows');
cell.hover = false;
button._syncTaskHover(cell, agent, provider, 3);
check(popups[0].destroyed, 'leaving removes the hover box');
button.menu.isOpen = true;
cell.hover = true;
button._syncTaskHover(cell, agent, provider, 3);
check(popups.length === 1, 'open detail menu suppresses hover');
button.menu.isOpen = false;
button._syncTaskHover(cell, agent, provider, 3);
const next = {hover: true};
button._syncTaskHover(next, {...agent, title: 'Other task'}, provider, 3);
check(popups[1].destroyed && popups[2].anchor === next, 'switching tasks removes the previous box');
cell.hover = false;
button._syncTaskHover(cell, agent, provider, 3);
check(popups[2].visible, 'late leave from old task does not close the new preview');
button._hideTaskHover();
button._hideTaskHover();
check(popups[2].destroyed, 'cleanup is safe with and without an active preview');
check(source.includes("cell.connect('notify::hover'") && source.includes("cell.connect('destroy'"),
    'real task actors wire hover and destruction cleanup');
print('PASS: anchored hover, task details, leave, task switch, menu suppression and cleanup.');
const [, cssBytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/stylesheet.css`).load_contents(null);
const css = new TextDecoder().decode(cssBytes);
check(/\.ams-task-hover-content\s*\{[^}]*background-color:[^}]*border:[^}]*padding:/s.test(css),
    'hover preview has an explicit separate box background, border and padding');
check(source.includes("item.add_style_class_name('ams-task-hover-content')"), 'preview applies the separate box style');
