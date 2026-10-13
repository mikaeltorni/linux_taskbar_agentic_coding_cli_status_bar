// Exercise the extension's public event dispatch without a live desktop.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '')
    .replace('export default class', 'class');
const Clutter = {EVENT_STOP: 1, EVENT_PROPAGATE: 0,
    EventType: {BUTTON_PRESS: 1, BUTTON_RELEASE: 2, TOUCH_BEGIN: 3, KEY_PRESS: 4, SCROLL: 5}};
class NativePanelButton {
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
const makeButton = new Function('Clutter', 'GObject', 'PanelMenu', 'Extension', 'print',
    `${source}\nreturn AccModelStatusButton;`);
const Button = makeButton(Clutter, {registerClass: value => value},
    {Button: NativePanelButton}, class {}, print);
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
