// Exercise the extension's public event dispatch without a live desktop.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '')
    .replace('export default class', 'class');
const Clutter = {EVENT_STOP: 1, EVENT_PROPAGATE: 0,
    EventType: {BUTTON_PRESS: 1, BUTTON_RELEASE: 2, TOUCH_BEGIN: 3, KEY_PRESS: 4, SCROLL: 5}};
class NativePanelButton {
    /** Initialize native parent defaults without desktop effects.
     * Parameters: this - isolated panel button; alignment - native menu alignment; name - accessible menu name.
     * Returns: undefined after installing the simulated native hover/menu properties.
     */
    _init(alignment, name) {
        print(`this=${this} alignment=${alignment} name=${name}`);
        this.track_hover = true;
        this.style = '';
        this.menu = {connect: () => 1};
        print(undefined);
    }
    add_child = child => { this.child = child; };

    /** Match GNOME PanelMenu.Button's existing pointer dispatch for the baseline.
     * Parameters: this - isolated panel button; event - incoming Clutter event.
     * Returns: the native event propagation value.
     */
    vfunc_event(event) {
        print(`this=${this} event=${event}`);
        if (this.menu && (event.type() === Clutter.EventType.TOUCH_BEGIN ||
            event.type() === Clutter.EventType.BUTTON_PRESS))
            this.menu.toggle();
        print(Clutter.EVENT_PROPAGATE);
        return Clutter.EVENT_PROPAGATE;
    }
}
const makeButton = new Function('Clutter', 'GObject', 'PanelMenu', 'Extension', 'print', 'Main', 'St', 'file',
    `${source}\npayloadFile = () => file; readUnreadStates = () => new Map(); return AccModelStatusButton;`);
const Button = makeButton(Clutter, {registerClass: value => value},
    {Button: NativePanelButton}, class {}, print,
    {panel: {menuManager: {_grabParams: {actionMode: 1}}}},
    {BoxLayout: class {}}, {get_parent: () => ({get_child: () => ({})})});
const button = new Button();
let menuOpen = false;
let toggles = 0;
button.menu = {toggle: () => { menuOpen = !menuOpen; toggles++; }};
const check = (condition, message) => { if (!condition) throw Error(message); };
const event = (type, mouseButton = 1) => ({type: () => type, get_button: () => mouseButton});
// The enclosing panel receives clicks on provider icons, labels, gaps and blank areas.
for (const mouseButton of [1, 2, 8]) {
    const before = toggles;
    button.vfunc_event(event(Clutter.EventType.BUTTON_PRESS, mouseButton));
    check(toggles === before && !menuOpen, 'only right-click opens the detailed submenu');
}
check(button.vfunc_event(event(Clutter.EventType.BUTTON_PRESS, 3)) === Clutter.EVENT_STOP,
    'right-click opens details and consumes the event');
check(menuOpen && toggles === 1, 'right-click exposes the existing menu');
for (const type of [Clutter.EventType.BUTTON_RELEASE, Clutter.EventType.SCROLL, Clutter.EventType.KEY_PRESS]) {
    check(button.vfunc_event(event(type, 3)) === Clutter.EVENT_PROPAGATE, 'non-press events preserve native propagation');
    check(menuOpen && toggles === 1, 'release and unrelated events do not close details');
}
button.vfunc_event(event(Clutter.EventType.BUTTON_PRESS, 3));
check(!menuOpen && toggles === 2, 'second right-click closes the same details menu');
button.vfunc_event(event(Clutter.EventType.TOUCH_BEGIN));
check(!menuOpen && toggles === 2, 'touch on the enclosing panel does not open details');
print('PASS: right-click details, left/middle/extra-button rejection, toggle and unrelated-event propagation.');

// Run the real initialization owner so whole-panel hover cannot be hidden by a CSS-only assertion.
const hoverButton = new Button();
hoverButton._startWatching = () => {};
hoverButton._startWindowWatching = () => {};
hoverButton._refresh = () => {};
hoverButton._init({dir: {get_child: () => ({})}});
check(hoverButton.track_hover === false, 'enclosing panel never tracks whole-bar hover');
check(hoverButton.style.includes('background-color: transparent') && hoverButton.style.includes('box-shadow: none'),
    'native active/focus/menu theme cannot highlight the entire indicator');
const [, cssBytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/stylesheet.css`).load_contents(null);
const css = new TextDecoder().decode(cssBytes);
const localHoverRule = css.match(/\.ams-panel-task-cell:hover[^{]*\{([^}]+)\}/);
check(localHoverRule && /background-color:\s*rgba\(/.test(localHoverRule[1]), 'hover background is scoped to the individual instance cell');
check(!/\.ams-panel-progress:hover|\.ams-panel-workspace:hover|\.ams-box:hover/.test(css),
    'hover styling does not cover neighboring instances or workspace labels');
print('PASS: actual panel initialization disables aggregate hover/active paint and CSS highlights only task cells.');
