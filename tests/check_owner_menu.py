"""Preserve earlier caller output when exporting an isolated owner selection."""
import json
import pty
import select
import time
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

def run_terminal(command, environment, keys):
    """Drive a real standalone component menu in an isolated terminal.

    Parameters: command - public installer argv; environment - isolated subprocess variables; keys - menu input bytes.
    Returns: exit status and complete terminal output.
    """
    print(f'command={command!r} environment={environment!r} keys={keys!r}')
    master, slave = pty.openpty()
    process = subprocess.Popen(command, env=environment, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    output = bytearray()
    sent = False
    saved = False
    deadline = time.monotonic() + 20
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                output.extend(chunk)
                if not sent and b'Named task workspaces' in output:
                    os.write(master, keys)
                    sent = True
                if not saved and b'Choice [1/2/3]' in output:
                    os.write(master, b'3\r')
                    saved = True
            elif process.poll() is not None:
                break
        if process.poll() is None:
            process.wait(timeout=2)
        assert sent, output.decode(errors='replace')
        result = process.returncode, output.decode(errors='replace')
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
    print(result)
    return result


ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='owner-export-stream-') as directory:
    temporary = Path(directory)
    checkout = temporary/'owner'
    checkout.mkdir()
    for relative in subprocess.check_output(['git','-C',str(ROOT),'ls-files'],text=True).splitlines():
        if relative in ('notes.json','notes.txt','used_notes.txt'):
            continue
        source = ROOT/relative
        if source.is_file() and not source.is_symlink():
            destination = checkout/relative
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(source,destination)
    shutil.copytree(ROOT/'.installer-support',checkout/'.installer-support',dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns('__pycache__','.log'))
    home = temporary/'home'
    home.mkdir()
    tools = temporary/'tools'
    tools.mkdir()
    effects = temporary/'effects'
    getent = tools/'getent'
    getent.write_text('#!/bin/bash\nprintf "%s:x:%s:%s::%s:/bin/bash\\n" "$USER" "$(id -u)" "$(id -g)" "$HOME"\n')
    getent.chmod(0o755)
    guard = '''#!/bin/bash
name="${0##*/}"
if [[ "$name" == gsettings ]]; then
 if [[ "${1:-}" == --schemadir ]]; then shift 2; fi
 case "${1:-}" in get|list-*|range|writable) exit 1 ;; esac
fi
if [[ "$name" == systemctl ]]; then
 while [[ "${1:-}" == --* ]]; do shift; done
 case "${1:-}" in is-active|is-enabled|status|show|list-unit-files|list-units) exit 1 ;; esac
fi
if [[ "$name" == gnome-extensions ]]; then
 case "${1:-}" in info|list) exit 1 ;; esac
fi
if [[ "$name" == tmux ]]; then
 case "${1:-}" in has-session|list-sessions|list-windows|list-panes|show-environment) exit 1 ;; esac
fi
printf '%s %s\\n' "$name" "$*" >> "$EFFECTS"
exit 97
'''
    for name in ('git','curl','wget','sudo','cargo','apt','apt-get','gsettings','dconf','systemctl','gnome-extensions','tmux','safe-system-action','xdotool','pkill','killall'):
        command = tools/name
        command.write_text(guard)
        command.chmod(0o755)
    # The real guarded power helper must never execute during a verification run.
    (checkout/'.installer-support/safe_system_action.sh').write_text('#!/bin/bash\nprintf "forbidden power helper\\n" >> "$EFFECTS"\nexit 97\n')
    environment = dict(PATH=str(tools)+':/usr/bin:/bin',HOME=str(home),USER=os.environ.get('USER','mk'),
                       XDG_CONFIG_HOME=str(home/'config'),XDG_STATE_HOME=str(home/'state'),
                       XDG_CACHE_HOME=str(home/'cache'),XDG_SESSION_TYPE='unknown',EFFECTS=str(effects),TERM='xterm')
    manifest = checkout/'installer/components.sh'
    core_name = 'install_agentic_status_core'
    with manifest.open('a') as stream:
        stream.write('\n# Record only fixture core work.\n# Parameters: none. Returns: zero or ninety-one for a requested core failure.\nCORE_NAME() { printf \'parameters=none\\n\' >&2; printf \'core\\n\' >> "$EFFECTS"; if [[ "${CORE_FAIL:-0}" == 1 ]]; then printf \'91\\n\' >&2; return 91; fi; printf \'0\\n\' >&2; }\n# Record installation through the actual public dispatcher.\n# Parameters: none. Returns: zero.\nfixture_install() { printf \'parameters=none\\n\' >&2; printf \'install\\n\' >> "$EFFECTS"; printf \'0\\n\' >&2; }\n# Record removal through the actual public dispatcher.\n# Parameters: none. Returns: zero.\nfixture_remove() { printf \'parameters=none\\n\' >&2; printf \'remove\\n\' >> "$EFFECTS"; printf \'0\\n\' >&2; }\n# Preserve unrelated fixture flags during lifecycle checks.\n# Parameters: none. Returns: zero.\nsync_feature_flags() { printf \'parameters=none\\n0\\n\' >&2; }\n# Exclude historical data migrations from this menu regression.\n# Parameters: none. Returns: zero.\nremove_legacy_speech_to_text_state() { printf \'parameters=none\\n0\\n\' >&2; }\nISC_COMPONENTS=("fixture|Named task workspaces|on|fixture_install||fixture_remove")\n'.replace('CORE_NAME',core_name))
    command = ['bash', str(checkout/'install.sh')]
    status, output = run_terminal(command, environment, b'q')
    assert status == 0 and not effects.exists(), ('cancel',status,output,effects.read_text() if effects.exists() else '')
    status, output = run_terminal(command, environment, b'\r')
    assert status == 0 and effects.read_text() == 'core\ninstall\n', ('install',status,output,effects.read_text() if effects.exists() else '')
    effects.unlink()
    status, output = run_terminal(command, environment, b'u\r')
    assert status == 0 and effects.read_text() == 'remove\n', ('remove',status,output,effects.read_text() if effects.exists() else '')
    effects.unlink()
    result = subprocess.run([*command,'--select','fixture'],cwd=checkout,env=environment,text=True,capture_output=True,timeout=30)
    assert result.returncode == 0 and effects.read_text() == 'core\ninstall\n', (result.stdout,result.stderr)
    effects.unlink()
    result = subprocess.run([*command,'--uninstall','fixture'],cwd=checkout,env=environment,text=True,capture_output=True,timeout=30)
    assert result.returncode == 0 and effects.read_text() == 'remove\n', (result.stdout,result.stderr)
    effects.unlink()
    environment['CORE_FAIL'] = '1'
    result = subprocess.run([*command,'--select','fixture'],cwd=checkout,env=environment,text=True,capture_output=True,timeout=30)
    assert result.returncode == 91 and effects.read_text() == 'core\n', ('core failure must stop component work',result.returncode,result.stdout,result.stderr)
print('PASS: owner menu cancel, accepted install and removal have exact public CLI effects')
