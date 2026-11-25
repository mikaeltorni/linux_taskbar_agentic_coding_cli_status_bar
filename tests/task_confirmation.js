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
        this.clutter_text = {};
        this.signals = {};
        this.actor = this;
        this.bin = {set_child: child => this.children.push(child)};
        print(undefined);
    }
    connect = (signal, callback) => { this.signals[signal] = callback; return signal; };
    disconnect = signal => { delete this.signals[signal]; };
    add_child = child => this.children.push(child);
    get_children = () => this.children;
    contains = actor => this.children.some(child => child === actor || child.contains?.(actor));
    add_style_class_name = () => {};
    set_style = style => { this.style = style; };
    remove_style_class_name = () => {};
    get_stage = () => ({});
    get_width = () => 200;
    get_height = () => 24;
    set_position = () => {};
    set_size = () => {};
    setPosition = actor => { this.anchor = actor; };
    open = () => { this.visible = true; };
    destroy = () => { this.destroyed = true; this.signals.destroy?.(this); };
}
const Clutter = {ModifierType: {CONTROL_MASK: 4, MOD1_MASK: 8, MOD5_MASK: 128}, EventType: {BUTTON_PRESS: 1, KEY_PRESS: 2},
    KEY_Escape: 27, KEY_Return: 13, KEY_KP_Enter: 14, EVENT_STOP: true, EVENT_PROPAGATE: false,
    ActorAlign: {CENTER: 0, FILL: 1, START: 2, END: 3}, BinLayout: class {}, FixedLayout: class {}};
const stage = new Actor();
stage.get_event_actor = event => event.target;
const popups = [];
const grabs = [];
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
        this.add_style_class_name = () => {};
        this.side = side;
        popups.push(this);
        print(undefined);
    }
};
const completed = [];
const focused = [];
const Button = new Function('GObject', 'PanelMenu', 'Extension', 'BoxPointer', 'St', 'Main', 'Clutter', 'global', 'print', 'PopupMenu', 'Shell', 'Pango', 'Gio',
    `${source}\nreturn {Button: AccModelStatusButton, buildPanelTaskCell, completionCommand};`)(
    {registerClass: value => value}, {Button: class {}}, class {},
    {BoxPointer: Pointer, PopupAnimation: {NONE: 0}},
    {Widget: Actor, BoxLayout: Actor, DrawingArea: Actor, Label: Actor, Icon: Actor, Button: Actor, Side: {BOTTOM: 3}},
    {layoutManager: {addChrome: () => {}},
        pushModal: actor => { const grab = {actor}; grabs.push(grab); return grab; },
        popModal: grab => { grabs.splice(grabs.indexOf(grab), 1); }},
    Clutter, {stage}, print, {PopupBaseMenuItem: Actor}, {ActionMode: {NORMAL: 1}}, {EllipsizeMode: {END: 3}}, Gio);
const button = new Button.Button();
button.menu = {isOpen: false};
button._agentMenuItem = (agent, provider, count, track) => new Actor({agent, provider, count, track});
button._completeAgent = agent => completed.push(agent);
const agent = {note_id: 'note-a', session_id: 'session-a', title: 'Fix test run behavior', phase: 'DONE'};
const provider = {kind: 'codex'};
let hides = 0;
const requestCompletion = (target, current) => button._showTaskConfirmation(target, current, provider, 3);
const cell = Button.buildPanelTaskCell(agent, id => focused.push(id), () => { hides++; }, false, requestCompletion);
const click = (actor, buttonNumber, modifiers = 0) => actor.signals['button-press-event'](actor,
    {type: () => Clutter.EventType.BUTTON_PRESS, get_button: () => buttonNumber, get_state: () => modifiers});
const arm = () => click(cell, 1, Clutter.ModifierType.MOD1_MASK);
const modalKey = key => stage.signals['captured-event'](stage,
    {type: () => Clutter.EventType.KEY_PRESS, get_key_symbol: () => key});
check(arm() === Clutter.EVENT_STOP && popups.length === 1, 'Left Alt-click opens confirmation on the taskbar cell');
check(focused.length === 0 && completed.length === 0, 'arming neither focuses nor completes');
check(popups[0].anchor === cell && popups[0].side === 3, 'confirmation sits above the exact instance');
check(grabs.length === 1 && grabs[0].actor === stage, 'confirmation grabs pointer and keyboard');
const popup = popups[0];
const content = popup.children[0];
check(content.children[0].agent === agent, 'confirmation includes the original colored task details');
check(content.children.some(child => /close this task and mark done/i.test(child.text || '')),
    'confirmation explicitly mentions closing the task and moving it to Done');
check(click(cell, 3) === Clutter.EVENT_PROPAGATE && completed.length === 0,
    'right-click on the task never completes it');
const actions = content.children.at(-1);
const confirm = actions.children.find(child => child.label === 'Close and mark Done');
check(stage.signals['captured-event'](stage, {type: () => Clutter.EventType.BUTTON_PRESS, target: confirm}) === Clutter.EVENT_PROPAGATE &&
    !popup.destroyed && completed.length === 0, 'popup button press is delivered without dismissing or completing');
confirm.signals.clicked();
check(completed.length === 1 && completed[0] === agent && popup.destroyed,
    'the explicit Done button completes the exact task once');
click(cell, 1);
check(focused.at(-1) === 'note-a', 'ordinary left-click retains terminal selection');
click(cell, 1, Clutter.ModifierType.MOD5_MASK);
check(popups.length === 1, 'AltGr does not open completion');
for (const modifiers of [Clutter.ModifierType.CONTROL_MASK, Clutter.ModifierType.CONTROL_MASK | Clutter.ModifierType.MOD1_MASK])
    click(cell, 1, modifiers);
check(hides === 2 && popups.length === 1, 'Ctrl-click retains the established hide gesture');
arm();
check(modalKey(Clutter.KEY_Return) === Clutter.EVENT_STOP && completed.length === 2,
    'Enter confirms a separately armed task');
arm();
const cancelled = popups.at(-1);
check(modalKey(Clutter.KEY_Escape) === Clutter.EVENT_STOP && cancelled.destroyed && completed.length === 2,
    'Escape cancels without completion');
arm();
const cancelledButton = popups.at(-1);
const cancel = cancelledButton.children[0].children.at(-1).children.find(child => child.label === 'Cancel');
cancel.signals.clicked();
check(cancelledButton.destroyed && completed.length === 2, 'Cancel button dismisses without completing');
arm();
const outside = popups.at(-1);
stage.signals['captured-event'](stage, {type: () => Clutter.EventType.BUTTON_PRESS, target: new Actor()});
check(outside.destroyed && completed.length === 2, 'outside click cancels without completion');
arm();
button._syncTaskHover({...cell, hover: false}, agent, provider, 3);
check(!popups.at(-1).destroyed, 'pointer leave does not dismiss the armed confirmation');
button._hideTaskConfirmation();
button._hideTaskConfirmation();
check(grabs.length === 0 && Object.keys(stage.signals).length === 0, 'cancellation releases modal input');
const anonymousCell = Button.buildPanelTaskCell({...agent, session_id: ''}, null, null, false, requestCompletion);
const previousPopups = popups.length;
click(anonymousCell, 1, Clutter.ModifierType.MOD1_MASK);
check(popups.length === previousPopups, 'the gesture cannot arm an unregistered task');
print('PASS: Left Alt-click opens an anchored confirmation with explicit buttons and keyboard handling.');

const registry = {sessions: {'session-a': {id: 'session-a', launch_env: {PHRASE_AUTOMATION_IN_PROGRESS_NOTE_ID: 'note-a', PHRASE_AUTOMATION_NOTES_FILE: '/tmp/isolated-notes.json'}}}};
const command = Button.completionCommand(agent, registry);
check(JSON.stringify(command) === JSON.stringify(['pa', 'mark-in-progress-done', '--close-session', 'session-a']), 'completion passes the exact registered task and notes document as argv');
for (const invalid of [{...agent, session_id: ''}, {...agent, session_id: '   '}, {...agent, session_id: null}]) {
    check(Button.completionCommand(invalid, registry) === null, 'missing or changed session identity cannot complete another task');
    check(registry.sessions['session-a'].launch_env.PHRASE_AUTOMATION_IN_PROGRESS_NOTE_ID === 'note-a', 'rejected identity preserves registered ownership');
}
check(JSON.stringify(Button.completionCommand(agent, {sessions: {}})) === JSON.stringify(['pa', 'mark-in-progress-done', '--close-session', 'session-a']), 'native owner validates missing frontend registry instead of frontend rejecting it');
print('PASS: completion uses registered note ownership; missing and mismatched identities are rejected.');

// The details menu no longer has a completion shortcut.
button._icon = () => ({});
button._focusAgent = id => focused.push(id);
button._taskRows = new Map();
button._runClockActors = [];
const row = Button.Button.prototype._agentMenuItem.call(button, agent, provider, 3);
const menuPopupCount = popups.length;
check(!row.signals['captured-event'], 'details row no longer captures the old Ctrl gesture');
row.signals.activate(row, {get_button: () => 1, get_state: () => Clutter.ModifierType.CONTROL_MASK});
check(popups.length === menuPopupCount && completed.length === 2, 'details-menu Ctrl-click cannot open confirmation or complete');
check(focused.at(-1) === 'note-a', 'details menu continues to select the exact task');
print('PASS: old details-menu completion shortcut removed.');

// The reported window has no launch-env note link; the panel carries its tmux name.
const recoveredAgent = {...agent, note_id: 'acc-session-a', tmux_name: 'acc-session-a'};
const recoveredRegistry = {sessions: {'session-a': {id: 'session-a', tmux_name: 'acc-session-a', launch_env: {}}}};
check(JSON.stringify(Button.completionCommand(recoveredAgent, recoveredRegistry)) ===
    JSON.stringify(['pa', 'mark-in-progress-done', '--close-session', 'session-a']),
    'unlinked taskbar session delegates note recovery to the native completion owner');
check(JSON.stringify(Button.completionCommand({...recoveredAgent, note_id: 'unrelated'}, recoveredRegistry)) === JSON.stringify(['pa', 'mark-in-progress-done', '--close-session', 'session-a']),
    'published label changes preserve the selected session identity for native validation');
print('PASS: reported unlinked session reaches native note recovery without a guessed note id.');

// Exercise the actual dispatch owner, including its registry file and subprocess boundary.
const dispatches = [];
const dispatchErrors = [];
const dispatchFile = {get_parent: () => ({get_child: part => {
    if (part !== 'sessions') throw Error('incorrect state parent');
    return {get_child: name => {
        if (name !== 'registry.json') throw Error('incorrect registry filename');
        return {get_path: () => '/isolated/state/sessions/registry.json',
            load_contents: () => [true, new TextEncoder().encode(JSON.stringify(recoveredRegistry))]};
    }};
}})};
const dispatchMain = {notifyError: (title, reason) => dispatchErrors.push({title, reason})};
let nativeOutcome = {success: true, stdout: '{"moved":true}', stderr: ''};
let nativePipeReads = 0;
const dispatchGio = {SubprocessFlags: {NONE: 0, STDOUT_PIPE: 4, STDERR_PIPE: 16}, Subprocess: {new: argv => {
    dispatches.push(argv);
    return {wait_check_async: (_cancel, callback) => callback({wait_check_finish: () => true}, {}),
        communicate_utf8_async: (_input, _cancel, callback) => {
            nativePipeReads++;
            callback({communicate_utf8_finish: () => [true, nativeOutcome.stdout, nativeOutcome.stderr],
                get_successful: () => nativeOutcome.success}, {});
        }};
}}};
const DispatchButton = new Function('GObject', 'PanelMenu', 'Extension', 'print', 'Gio', 'Main',
    `${source}\nreturn AccModelStatusButton;`)(
    {registerClass: value => value}, {Button: class {}}, class {}, print, dispatchGio, dispatchMain);
const liveDispatch = new DispatchButton();
liveDispatch._file = dispatchFile;
let dispatchRefreshes = 0;
liveDispatch._queueRefresh = () => { dispatchRefreshes++; };
liveDispatch._completeAgent(recoveredAgent);
check(dispatchErrors.length === 0 && dispatches.length === 1 && dispatchRefreshes === 1,
    'actual pre-dispatch owner reads the expected registry and invokes native session recovery');
check(JSON.stringify(dispatches[0]) === JSON.stringify(['pa', 'mark-in-progress-done', '--close-session', 'session-a']),
    'full dispatch sends the selected recovered session to pa');
print('PASS: actual completion owner, registry path, subprocess boundary and completion refresh.');

// Frontend identity lookup must never block the native session ownership resolver.
liveDispatch._file = {get_parent: () => ({get_child: () => ({get_child: () => ({
    get_path: () => '/isolated/frontend/sessions/registry.json',
    load_contents: () => [true, new TextEncoder().encode('{"sessions":{}}')],
})})})};
const beforeNativeValidation = dispatches.length;
liveDispatch._completeAgent(recoveredAgent);
check(dispatches.length === beforeNativeValidation + 1 && dispatchErrors.length === 0,
    'the photographed frontend note-match error cannot block stable-session dispatch');
print('PASS: native ownership validation remains authoritative when frontend registry has no note match.');

const beforeFailureRefresh = dispatchRefreshes;
nativeOutcome = {success: false, stdout: '{"moved":false}', stderr: 'parameters=debug\nSelected ACC session no longer exists\n'};
liveDispatch._completeAgent(recoveredAgent);
check(dispatchErrors.at(-1)?.reason === 'Selected ACC session no longer exists',
    'native ownership error is shown instead of a generic subprocess status');
check(dispatchRefreshes === beforeFailureRefresh && nativePipeReads === 3,
    'failed completion drains both output pipes and never reports success');
nativeOutcome = {success: false, stdout: '', stderr: ''};
liveDispatch._completeAgent(recoveredAgent);
check(dispatchErrors.at(-1)?.reason === 'Native task completion failed', 'empty native stderr has a useful fallback');
check(dispatchRefreshes === beforeFailureRefresh, 'empty native failure does not refresh as success');
const beforeMissingId = dispatches.length;
liveDispatch._completeAgent({...recoveredAgent, session_id: ''});
check(dispatches.length === beforeMissingId && dispatchRefreshes === beforeFailureRefresh,
    'missing session id still rejects before starting a subprocess');
print('PASS: native output pipes, precise failures, missing-id rejection and success-only refresh.');

// Hover the real task row with Vibe's legacy generic provider metadata.
delete button._agentMenuItem;
button.menu.isOpen = false;
button._iconsDir = Gio.File.new_for_path(`${root}/acc-model-status@local/icons`);
const genericIcon = new Gio.FileIcon({file: button._iconsDir.get_child('acc-provider-generic-symbolic.svg')});
button._icon = () => genericIcon;
const projectAgent = {...agent, project_tag: 'ACC'};
const hoverTarget = {hover: true};
for (const [kind, expected] of [['vibe', 'mistral.svg'], ['unknown', 'acc-provider-generic-symbolic.svg']]) {
    button._syncTaskHover(hoverTarget, {...projectAgent, kind}, {
        kind, icon: 'acc-provider-generic-symbolic',
    }, 1);
    const stack = popups.at(-1).children[0].children[0];
    const [tag, logo, badge] = stack.children[1].children[0].children;
    check(tag.text === 'ACC' && logo.gicon.get_file().get_basename() === expected && badge.text === 'DONE',
        `hover for ${kind} places ${expected} between the project tag and phase`);
    check(logo.gicon.get_file().query_exists(null), `hover asset exists for ${kind}`);
}
button._hideTaskHover();
