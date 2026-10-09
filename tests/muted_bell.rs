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
    let reader = method(&source, "readPayload");
    let refresh = method(&source, "_refresh");
    let early = refresh.find("if (this.menu.isOpen)").unwrap();
    assert!(refresh[early..].find("this._syncMuteIndicator").unwrap() < refresh[early..].find("return;").unwrap());
    let clear = refresh.find("destroy_all_children").unwrap();
    assert!(refresh[clear..].find("this._syncMuteIndicator").unwrap() < refresh[clear..].find("this._renderEmpty").unwrap());
    assert!(source.contains("announcementsMuted: parsed.announcements_muted === true"));
    let script = format!(r#"
const emit = print; print = () => {{}};
{reader}
const Clutter = {{ActorAlign: {{CENTER: 0}}}};
const St = {{Icon: class {{ constructor(props) {{
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
const button = {{_box: box, _muteIcon: null, {sync}}};
button._syncMuteIndicator(false);
const unmuted = box.children.length;
button._syncMuteIndicator(true);
const icon = box.children[0];
const muted = [icon.icon_name, icon.visible, box.children[1] === provider];
button._syncMuteIndicator(false);
const restored = [icon.visible, box.children[1] === provider];
button._syncMuteIndicator(true);
emit(JSON.stringify({{flags, absent, unmuted, muted, restored, same: box.children[0] === icon}}) + '\n');
"#);
    let result = Command::new("gjs").args(["-c", &script]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let observed: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(observed, serde_json::json!({"flags": [false, true, false, false], "absent": null, "unmuted": 1, "muted": ["notifications-disabled-symbolic", true, true], "restored": [false, true], "same": true}));
    eprintln!("None");
}
