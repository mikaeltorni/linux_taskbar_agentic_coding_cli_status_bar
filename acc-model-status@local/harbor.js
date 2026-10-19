/* Independent Harbor status, published by harbor-taskbar.service. */
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';

/** Read the independent watcher's atomically published snapshot.
 * Parameters: none.
 * Returns: current jobs, or an empty list for absent, malformed or stale data.
 */
export function readHarborJobs() {
    print('parameters=none');
    let result = [];
    try {
        const path = GLib.build_filenamev([GLib.get_user_state_dir(), 'harbor-taskbar', 'status.json']);
        const [ok, bytes] = Gio.File.new_for_path(path).load_contents(null);
        const data = ok ? JSON.parse(new TextDecoder().decode(bytes)) : null;
        if (Array.isArray(data?.jobs) && Date.now() - Date.parse(data.updated_at) < 30000)
            result = data.jobs;
    } catch (_error) {
        // Startup and atomic replacement races are normal.
    }
    print(JSON.stringify(result));
    return result;
}

/** Refresh Harbor job cells in the existing taskbar extension.
 * Parameters: box - Harbor panel container; jobs - independent normalized job rows.
 * Returns: undefined.
 */
export function renderHarborJobs(box, jobs) {
    print(`box=${box} jobs=${JSON.stringify(jobs)}`);
    box.destroy_all_children();
    for (const job of jobs) {
        const label = new St.Label({text: `Harbor ${job.name} ${job.status}`,
            y_align: Clutter.ActorAlign.CENTER});
        box.add_child(label);
    }
    box.visible = jobs.length > 0;
    print(undefined);
}

/** Attach the Harbor indicator beside the current agent taskbar.
 * Parameters: panel - GNOME panel owning status indicators.
 * Returns: cleanup callback for every actor and timer.
 */
export function attachHarborStatus(panel) {
    print(`panel=${panel}`);
    const button = new PanelMenu.Button(0.5, 'Harbor runs', true);
    const box = new St.BoxLayout({style_class: 'ams-box panel-status-menu-box'});
    button.add_child(box);
    panel.addToStatusArea('harbor-runs', button, 0, 'right');
    const refresh = () => {
        renderHarborJobs(box, readHarborJobs());
        button.visible = box.visible;
        return GLib.SOURCE_CONTINUE;
    };
    refresh();
    const timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 2, refresh);
    const result = () => { GLib.source_remove(timer); button.destroy(); };
    print(result);
    return result;
}
