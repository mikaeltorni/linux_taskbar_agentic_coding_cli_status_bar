// Exercise the extension's real click and popup owners without desktop effects.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const check = (condition, message) => { if (!condition) throw Error(message); };
class Actor {
    /** Build a detached actor stub.
     * Parameters: this - actor; options - initial properties.
     * Returns: undefined.
     */
    constructor(options = {}) {
        print(`this=${this} options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        this.children = [];
        this.signals = {};
        this.bin = {set_child: child => this.children.push(child)};
        print(undefined);
    }
    connect = (signal, callback) => { this.signals[signal] = callback; return signal; };
    disconnect = signal => { delete this.signals[signal]; };
    add_child = child => this.children.push(child);
    get_children = () => this.children;
    add_style_class_name = () => {};
    set_style = style => { this.style = style; };
    setPosition = actor => { this.anchor = actor; };
    open = () => { this.visible = true; };
    destroy = () => { this.destroyed = true; this.signals.destroy?.(this); };
}
const Clutter = {ModifierType: {CONTROL_MASK: 4}, EventType: {BUTTON_PRESS: 1, KEY_PRESS: 2},
    KEY_Escape: 27, EVENT_STOP: true, EVENT_PROPAGATE: false,
    ActorAlign: {CENTER: 0, FILL: 1, START: 2, END: 3}, BinLayout: class {}};
const stage = new Actor();
const popups = [];
const Pointer = class {
    /** Record an anchored popup.
     * Parameters: this - popup; side - arrow edge; options - actor properties.
     * Returns: undefined.
     */
    constructor(side, options) {
        print(`this=${this} side=${side} options=${JSON.stringify(options)}`);
        Object.assign(this, new Actor(options));
        this.setPosition = actor => { this.anchor = actor; };
        this.open = () => { this.visible = true; };
        this.destroy = () => { this.destroyed = true; };
        this.contains = actor => this.children.includes(actor) || this.children.some(child => child.children.includes(actor));
        this.add_style_class_name = () => {};
        this.side = side;
        popups.push(this);
        print(undefined);
    }
};
const completed = [];
const focused = [];
const Button = new Function('GObject', 'PanelMenu', 'Extension', 'BoxPointer', 'St', 'Main', 'Clutter', 'global', 'print',
    `${source}\nreturn {Button: AccModelStatusButton, buildPanelTaskCell, completionCommand};`)(
    {registerClass: value => value}, {Button: class {}}, class {},
    {BoxPointer: Pointer, PopupAnimation: {NONE: 0}},
    {Widget: Actor, BoxLayout: Actor, DrawingArea: Actor, Label: Actor, Side: {BOTTOM: 3}},
    {layoutManager: {addChrome: () => {}}}, Clutter, {stage}, print);
const button = new Button.Button();
button.menu = {isOpen: false};
button._agentMenuItem = (agent, provider, count, track) => new Actor({agent, provider, count, track});
button._completeAgent = agent => completed.push(agent);
const agent = {note_id: 'note-a', session_id: 'session-a', title: 'Fix test run behavior', phase: 'DONE'};
const provider = {kind: 'codex'};
const cell = Button.buildPanelTaskCell(agent, id => focused.push(id),
    (target, current) => button._showTaskConfirmation(target, current, provider, 3));
const click = (actor, buttonNumber, ctrl = false) => actor.signals['button-press-event'](actor,
    {get_button: () => buttonNumber, get_state: () => ctrl ? 4 : 0});
check(click(cell, 1, true) === Clutter.EVENT_STOP, 'Ctrl+left-click consumes the panel event');
check(focused.length === 0 && completed.length === 0, 'arming neither focuses nor completes');
check(popups.length === 1 && popups[0].anchor === cell && popups[0].side === 3, 'confirmation sits above the exact instance');
const popup = popups[0];
const content = popup.children[0];
check(content.children[0].agent === agent, 'confirmation includes the original colored task details');
check(content.children.some(child => /right.click.*confirm/i.test(child.text || '')), 'visible text says right-click to confirm');
for (const mouseButton of [1, 2, 8]) {
    click(content, mouseButton);
    check(completed.length === 0, 'other buttons never confirm');
}
click(cell, 3);
check(completed.length === 1 && completed[0] === agent && popup.destroyed, 'right-click on the instance confirms exact task once');
click(cell, 3);
check(completed.length === 1, 'a second right-click never repeats completion');
click(cell, 1);
check(focused.at(-1) === 'note-a', 'ordinary left-click retains terminal selection');
click(cell, 1, true);
click(popups.at(-1).children[0], 3);
check(completed.length === 2, 'right-click on the confirmation also works');
click(cell, 1, true);
const cancelled = popups.at(-1);
stage.signals['captured-event'](stage, {type: () => 2, get_key_symbol: () => 27});
check(cancelled.destroyed && completed.length === 2, 'Escape cancels without completion');
click(cell, 1, true);
const outside = popups.at(-1);
stage.signals['captured-event'](stage, {type: () => 1, get_source: () => new Actor()});
check(outside.destroyed && completed.length === 2, 'outside click cancels without completion');
click(cell, 1, true);
button._syncTaskHover({...cell, hover: false}, agent, provider, 3);
check(!popups.at(-1).destroyed, 'pointer leave does not dismiss the armed confirmation');
button._hideTaskConfirmation();
button._hideTaskConfirmation();
check(Object.keys(stage.signals).length === 0, 'cancellation releases stage input handler');
const missing = Button.buildPanelTaskCell({}, id => focused.push(id), () => { throw Error('must not arm missing task'); });
click(missing, 1, true);
check(completed.length === 2, 'missing task identity cannot arm confirmation');
print('PASS: Ctrl+click, anchored task details, right-click confirmation, other-button rejection, cancellation and ordinary selection.');

const registry = {sessions: {'session-a': {id: 'session-a', launch_env: {PHRASE_AUTOMATION_IN_PROGRESS_NOTE_ID: 'note-a', PHRASE_AUTOMATION_NOTES_FILE: '/tmp/isolated-notes.json'}}}};
const command = Button.completionCommand(agent, registry);
check(JSON.stringify(command) === JSON.stringify(['pa', 'mark-in-progress-done', '--id', 'note-a', '--notes-file', '/tmp/isolated-notes.json']), 'completion passes the exact registered task and notes document as argv');
for (const invalid of [{...agent, session_id: 'missing'}, {...agent, note_id: 'other'}, {...agent, session_id: ''}]) {
    check(Button.completionCommand(invalid, registry) === null, 'missing or changed session identity cannot complete another task');
    check(registry.sessions['session-a'].launch_env.PHRASE_AUTOMATION_IN_PROGRESS_NOTE_ID === 'note-a', 'rejected identity preserves registered ownership');
}
check(Button.completionCommand(agent, {sessions: {}}) === null, 'absent registry rejects completion');
print('PASS: completion uses registered note ownership; missing and mismatched identities are rejected.');
