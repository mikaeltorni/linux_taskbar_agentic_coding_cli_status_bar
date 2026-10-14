// Run the actual GIO async completion boundary against a disposable native command.
const Gio = imports.gi.Gio;
const GLib = imports.gi.GLib;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const check = (condition, message) => { if (!condition) throw Error(message); };
const temporary = GLib.dir_make_tmp('acc-panel-complete-XXXXXX');
const bin = `${temporary}/bin`;
GLib.mkdir_with_parents(bin, 0o700);
const executable = `${bin}/pa`;
const argumentFile = `${temporary}/arguments`;
const script = `#!/bin/sh
printf '%s\\n' "$@" > "$ACC_COMPLETION_PROBE_ARGS"
if [ "$ACC_COMPLETION_PROBE_RESULT" = fail ]; then
    printf 'parameters=debug\\nSelected ACC session no longer exists\\n' >&2
    exit 1
fi
printf '{"moved":true,"note_id":"selected-note"}\\n'
`;
GLib.file_set_contents(executable, script);
GLib.chmod(executable, 0o700);
const originalPath = GLib.getenv('PATH');
GLib.setenv('PATH', `${bin}:${originalPath}`, true);
GLib.setenv('ACC_COMPLETION_PROBE_ARGS', argumentFile, true);
const errors = [];
let refreshes = 0;
let loop;
const main = {notifyError: (title, reason) => { errors.push({title, reason}); loop.quit(); }};
const Button = new Function('GObject', 'PanelMenu', 'Extension', 'print', 'Gio', 'Main',
    `${source}\nreturn AccModelStatusButton;`)(
    {registerClass: value => value}, {Button: class {}}, class {}, print, Gio, main);
const button = new Button();
button._queueRefresh = () => { refreshes++; loop.quit(); };
try {
    for (const result of ['success', 'fail']) {
        GLib.setenv('ACC_COMPLETION_PROBE_RESULT', result, true);
        loop = new GLib.MainLoop(null, false);
        let timedOut = false;
        const timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 3000, () => {
            timedOut = true;
            loop.quit();
            return GLib.SOURCE_REMOVE;
        });
        button._completeAgent({session_id: 'selected-session', note_id: 'legacy-tmux-placeholder'});
        loop.run();
        if (!timedOut)
            GLib.source_remove(timeout);
        check(!timedOut, 'GIO completion callback runs without blocking the Shell');
        const [, argsBytes] = Gio.File.new_for_path(argumentFile).load_contents(null);
        check(new TextDecoder().decode(argsBytes) === 'mark-in-progress-done\n--close-session\nselected-session\n',
            'real subprocess receives only the selected stable session id');
        check(refreshes === 1, 'native failure cannot refresh as a successful completion');
        if (result === 'success')
            check(errors.length === 0, 'native completion succeeds through actual GIO pipes');
        else
            check(errors.length === 1 && errors[0].reason === 'Selected ACC session no longer exists',
                'real stderr pipe preserves the native ownership error');
    }
    print('PASS: real GIO async subprocess, selected-session argv, drained pipes, native error and success-only refresh.');
} finally {
    GLib.setenv('PATH', originalPath, true);
    for (const file of [argumentFile, executable])
        Gio.File.new_for_path(file).delete(null);
    Gio.File.new_for_path(bin).delete(null);
    Gio.File.new_for_path(temporary).delete(null);
}
