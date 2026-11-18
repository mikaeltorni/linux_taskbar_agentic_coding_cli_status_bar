// Exercise actual GObject initialization without starting a desktop session.
const Gio = imports.gi.Gio;
const GObject = imports.gi.GObject;
const [, bytes] = Gio.File.new_for_path(`${ARGV[0]}/agentic-cli-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;$/gm, '').replace('export default class', 'class');
class Actor {
    /** Create an isolated actor.
     * Parameters: self - actor instance; options - actor properties.
     * Returns: None.
     */
    constructor(options = {}) {
        print(`self=${this}, options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        this.children = [];
        print(null);
    }
    add_child = actor => this.children.push(actor);
}
const Parent = GObject.registerClass(class CodexStatusNativeParent extends GObject.Object {
    /** Initialize the native button parent.
     * Parameters: self - button instance; alignment - menu alignment; name - button name.
     * Returns: None.
     */
    _init(alignment, name) {
        print(`self=${this}, alignment=${alignment}, name=${name}`);
        super._init();
        print(null);
    }
    /** Attach an actor to the native button.
     * Parameters: self - button instance; actor - child actor.
     * Returns: None.
     */
    add_child(actor) {
        print(`self=${this}, actor=${actor}`);
        this.child = actor;
        print(null);
    }
});
const Button = new Function('GObject', 'PanelMenu', 'Extension', 'St', 'Clutter',
    `${source}\nreturn AgenticCliStatusButton;`)(GObject, {Button: Parent}, class {},
    {BoxLayout: Actor, Label: Actor}, {ActorAlign: {CENTER: 0}});
const button = new Button();
if (!(button._codexTracking instanceof Map) || button._segments.size !== 7 || !button._segments.has('vibe'))
    throw Error('Codex tracking and all status labels must initialize on a native button');
print('PASS: Codex status button initializes on a real GObject parent.');
