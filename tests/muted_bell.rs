//! Exercise muted-bell rendering without loading GNOME Shell or opening windows.
use std::{fs, process::Command};

/// Extract a real extension method for an isolated actor simulation.
/// Parameters: source - extension text; name - method name.
/// Returns: complete method source.
fn method(source: &str, name: &str) -> String {
    eprintln!("source={source} name={name}");
    let needle = format!("\n    {name}(");
    let start = source.find(&needle).or_else(|| source.find(&format!("\nfunction {name}("))).expect("requested method exists") + 1;
    let mut depth = 0;
    for (offset, ch) in source[start..].char_indices() {
        if ch == '{' { depth += 1; }
        if ch == '}' {
            depth -= 1;
            if depth == 0 {
                let result = source[start..=start + offset].to_string();
                eprintln!("{result}");
                return result;
            }
        }
    }
    panic!("unclosed method");
}

/// Check the actual renderer preserves actors and places the bell first.
/// Parameters: none.
/// Returns: None.
#[test]
fn muted_bell_is_leftmost_and_updates_without_replacing_providers() {
    eprintln!("parameters=none");
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/acc-model-status@local/extension.js")).unwrap();
    let sync = method(&source, "_syncMuteIndicator");
    let toggle = method(&source, "_toggleAnnouncements");
    let reader = method(&source, "readPayload");
    let refresh = method(&source, "_refresh");
    let early = refresh.find("if (this.menu.isOpen)").unwrap();
    assert!(refresh[early..].find("this._syncMuteIndicator").unwrap() < refresh[early..].find("return;").unwrap());
    let clear = refresh.find("destroy_all_children").unwrap();
    assert!(refresh[clear..].find("this._syncMuteIndicator").unwrap() < refresh[clear..].find("this._renderEmpty").unwrap());
    assert!(source.contains("announcementsMuted: parsed.announcements_muted === true"));
    assert!(refresh.contains("child !== this._muteButton"));
    let script = format!(r#"
const emit = print; print = () => {{}};
imports.gi.versions.Gtk = '3.0';
const Gtk = imports.gi.Gtk;
const theme = new Gtk.IconTheme(); theme.set_custom_theme('Adwaita');
const check = (condition, message) => {{ if (!condition) throw Error(message); }};
{reader}
const Clutter = {{ActorAlign: {{CENTER: 0}}}};
const calls = []; const errors = []; let failed = false; let exitFailed = false;
const GLib = {{get_home_dir: () => '/test', build_filenamev: parts => parts.join('/')}};
const Gio = {{SubprocessFlags: {{NONE: 0}}, Subprocess: {{new: (argv, flags) => {{
    if (failed) throw new Error('unavailable');
    calls.push(argv);
    return {{wait_check_async: (_cancel, callback) => callback({{wait_check_finish: () => {{ if (exitFailed) throw new Error('exit 1'); return true; }}}}, null)}};
}} }} }};
const logError = (error, message) => errors.push(message);
const St = {{Button: class {{ constructor(props) {{
    // Construct a button. Parameters: this - receiver; props - actor options. Returns: undefined.
    print(`this=${{this}}, props=${{JSON.stringify(props)}}`);
    Object.assign(this, props); this.signals = {{}}; this.visible = true;
    print(undefined);
}} set_child(child) {{
    // Attach the bell. Parameters: this - receiver; child - icon. Returns: undefined.
    print(`this=${{this}}, child=${{child}}`); this.child = child; print(undefined);
}} connect(signal, callback) {{
    // Save click handler. Parameters: this - receiver; signal - name; callback - handler. Returns: undefined.
    print(`this=${{this}}, signal=${{signal}}, callback=${{callback}}`); this.signals[signal] = callback; print(undefined);
}} }}, Icon: class {{ constructor(props) {{
    // Initialize an icon stub. Parameters: this - icon receiver; props - actor options. Returns: undefined.
    print(`this=${{this}}, props=${{JSON.stringify(props)}}`);
    Object.assign(this, props);
    print(undefined);
}} }} }};
const file = value => ({{load_contents: () => [true, new TextEncoder().encode(JSON.stringify(value))]}});
const flags = [{{}}, {{announcements_muted: true}}, {{announcements_muted: false}}, {{announcements_muted: '1'}}].map(value => readPayload(file(value)).announcementsMuted);
const absent = readPayload({{load_contents: () => [false, null]}});
const provider = {{name: 'codex'}};
const box = {{children: [provider], insert_child_at_index(actor, index) {{
    // Insert one panel actor. Parameters: this - box receiver; actor - widget; index - insertion position. Returns: undefined.
    print(`this=${{this}}, actor=${{JSON.stringify(actor)}}, index=${{index}}`);
    this.children.splice(index, 0, actor);
    print(undefined);
}} }};
const button = {{_box: box, _muteIcon: null, _muteButton: null, {sync}, {toggle}}};
button._syncMuteIndicator(false);
const unmuted = [box.children.length, button._muteIcon.icon_name, box.children[0].visible];
check(theme.has_icon(button._muteIcon.icon_name), 'unmuted bell must resolve in the desktop icon theme');
check(box.children[0].accessible_name === 'Mute ACC announcements', 'unmuted button describes its action');
check(box.children[0].style_class === 'ams-mute-button', 'button has a stable click target style');
box.children[0].signals.clicked();
button._syncMuteIndicator(true);
check(theme.has_icon(button._muteIcon.icon_name), 'muted bell must resolve in the desktop icon theme');
check(box.children[0].accessible_name === 'Unmute ACC announcements', 'muted button describes its action');
const actor = box.children[0];
const icon = button._muteIcon;
actor.signals.clicked();
const muted = [icon.icon_name, icon.visible, box.children[1] === provider];
button._syncMuteIndicator(false);
const restored = [icon.icon_name, actor.visible, box.children[1] === provider];
exitFailed = true; actor.signals.clicked();
failed = true; actor.signals.clicked();
button._syncMuteIndicator(true);
emit(JSON.stringify({{flags, absent, unmuted, muted, restored, same: box.children[0] === actor, calls, errors}}) + '\n');
"#);
    let result = Command::new("gjs").args(["-c", &script]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let observed: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(observed, serde_json::json!({"flags": [false, true, false, false], "absent": null, "unmuted": [2, "preferences-system-notifications-symbolic", true], "muted": ["notifications-disabled-symbolic", true, true], "restored": ["preferences-system-notifications-symbolic", true, true], "same": true, "calls": [["/test/.local/bin/agent-command-center", "silent", "toggle"], ["/test/.local/bin/agent-command-center", "silent", "toggle"], ["/test/.local/bin/agent-command-center", "silent", "toggle"]], "errors": ["acc-model-status: could not toggle announcements", "acc-model-status: could not toggle announcements"]}));
    let css = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/acc-model-status@local/stylesheet.css")).unwrap();
    assert!(css.contains(".ams-mute-button {"));
    assert!(css.contains("min-width: 24px;") && css.contains("min-height: 24px;"));
    assert!(css.contains(".ams-mute-button:hover"));
    eprintln!("None");
}
