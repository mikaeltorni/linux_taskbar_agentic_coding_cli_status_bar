// Exercise actual GObject initialization: JavaScript class fields are not ready in _init.
const Gio = imports.gi.Gio;
const GObject = imports.gi.GObject;
const [, bytes] = Gio.File.new_for_path(`${ARGV[0]}/acc-model-status@local/extension.js`).load_contents(null);
// The headless parent is native GObject rather than Clutter, so omit only its unavailable event vfunc.
const source = new TextDecoder().decode(bytes).replace(/^import .*;$/gm, '').replace('export default class', 'class')
    .replace(/\n    vfunc_event\(event\) \{[\s\S]*?\n    \}/, '');
class Actor {
    /** Initialize an isolated container without desktop effects.
     * Parameters: this - mock container; options - actor properties.
     * Returns: undefined.
     */
    constructor(options = {}) {
        print(`this=${this} options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        this.children = [];
        print(undefined);
    }
    add_child = actor => { actor.parent = this; this.children.push(actor); };
    get_parent = () => this.parent;
    get_children = () => this.children;
    destroy_all_children = () => { this.children = []; };
}
const NativePanelButton = GObject.registerClass(class HarborNativeParent extends GObject.Object {
    /** Initialize the real native base before the extension's own startup.
     * Parameters: this - native receiver; alignment - menu alignment; name - menu name.
     * Returns: undefined.
     */
    _init(alignment, name) {
        print(`this=${Object.prototype.toString.call(this)} alignment=${alignment} name=${name}`);
        super._init();
        this.menu = {connect: () => 1};
        print(undefined);
    }
    /** Accept the extension's initial container.
     * Parameters: this - native parent; actor - panel container.
     * Returns: undefined.
     */
    add_child(actor) {
        print(`this=${this} actor=${actor}`);
        this.child = actor;
        print(undefined);
    }
});
const Panel = new Function('GObject', 'PanelMenu', 'Extension', 'St', 'Main', 'file', `${source};
    payloadFile = () => file; readUnreadStates = () => new Map(); return AccModelStatusButton;`)(
    GObject, {Button: NativePanelButton}, class {}, {BoxLayout: Actor},
    {panel: {menuManager: {_grabParams: {actionMode: 1}}}}, {get_parent: () => ({get_child: () => ({})})});
Panel.prototype._startWatching = () => {};
Panel.prototype._startWindowWatching = () => {};
Panel.prototype._syncPanelPulseTimer = () => {};
Panel.prototype._refresh = function () { this._updatePanelProgress(null, false); };
const panel = new Panel({dir: {get_child: () => ({})}});
if (!panel._panelProgress || panel._panelProgress.visible !== false)
    throw Error('Common panel renderer must be callable during real GObject _init before class fields initialize');
print('PASS: common panel renderer is available during native GObject initialization.');
