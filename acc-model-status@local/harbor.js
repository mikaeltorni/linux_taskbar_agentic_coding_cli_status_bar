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
export const readHarborJobs = () => {
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
    return result;
};

/** Refresh compact pink Harbor cells, retaining their actors across polling cycles.
 * Parameters: box - Harbor container; jobs - normalized jobs; windows - terminal window descriptors.
 * Returns: undefined.
 */
export const renderHarborJobs = (box, jobs, windows = []) => {
    box._harborCells ??= new Map();
    const alive = new Set();
    for (const job of jobs) {
        alive.add(job.id);
        let widgets = box._harborCells.get(job.id);
        if (!widgets) {
            const group = new St.BoxLayout({style: 'spacing: 3px; margin-left: 4px;',
                y_expand: true, y_align: Clutter.ActorAlign.FILL});
            const workspace = new St.Label({style: 'color: #f472b6; font-size: 10px;', y_align: Clutter.ActorAlign.CENTER});
            const cell = new St.BoxLayout({vertical: true, width: 64, height: 32,
                y_expand: true, y_align: Clutter.ActorAlign.FILL});
            const count = new St.Label({style: 'color: #f472b6; font-size: 11px;',
                x_expand: true, y_expand: true,
                x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.START});
            const bar = new St.DrawingArea({width: 60, height: 6,
                x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.CENTER});
            const estimate = new St.Label({style: 'color: #c0bfbc; font-size: 10px;',
                x_expand: true, y_expand: true,
                x_align: Clutter.ActorAlign.CENTER, y_align: Clutter.ActorAlign.END});
            cell.add_child(count);
            cell.add_child(bar);
            cell.add_child(estimate);
            group.add_child(workspace);
            group.add_child(cell);
            box.add_child(group);
            widgets = {group, workspace, count, bar, estimate};
            bar.connect('repaint', area => paintHarborBar(area, widgets.job));
            box._harborCells.set(job.id, widgets);
        }
        widgets.job = job;
        const workspace = workspaceForHarbor(job, windows);
        widgets.workspace.text = workspace > 0 ? `H W${workspace}` : 'H W?';
        const fraction = harborProgress(job);
        widgets.count.text = fraction === null ? '?/?' : `${job.completed}/${job.total}`;
        widgets.estimate.text = formatHarborEstimate(job);
        widgets.group.accessible_name = `Harbor ${job.name}: ${widgets.count.text}, ${job.status}, ${widgets.estimate.text}`;
        widgets.bar.queue_repaint();
    }
    for (const [id, widgets] of box._harborCells) {
        if (!alive.has(id)) { widgets.group.destroy(); box._harborCells.delete(id); }
    }
    box.visible = jobs.length > 0;
};

/** Validate published counts before calculating the painted progress fraction.
 * Parameters: job - normalized job progress.
 * Returns: bounded completed fraction or null for unknown progress.
 */
export const harborProgress = (job) => {
    const result = Number.isInteger(job.completed) && Number.isInteger(job.total) &&
        job.total > 0 && job.completed >= 0 && job.completed <= job.total
        ? job.completed / job.total : null;
    return result;
};

/** Paint Harbor's measured progress without placing text inside the bar.
 * Parameters: area - allocated drawing area; job - current job counts and state.
 * Returns: undefined.
 */
export const paintHarborBar = (area, job) => {
    const [width, height] = area.get_surface_size();
    const context = area.get_context();
    context.setSourceRGBA(0.28, 0.27, 0.29, 1);
    context.rectangle(0, 0, width, height);
    context.fill();
    const fraction = harborProgress(job);
    if (fraction !== null) {
        if (job.status === 'interrupted')
            context.setSourceRGBA(0.94, 0.27, 0.27, 1);
        else
            context.setSourceRGBA(0.957, 0.447, 0.714, 1);
        context.rectangle(0, 0, Math.round(width * fraction), height);
        context.fill();
    }
    context.$dispose();
};

/** Match actual owner windows without borrowing desktop focus or ambiguous clients.
 * Parameters: job - terminal owner pids; windows - actual one-based workspace descriptors.
 * Returns: unique workspace number or zero when unknown, sticky or ambiguous.
 */
export const workspaceForHarbor = (job, windows) => {
    const matches = windows.filter(window => job.owner_pids?.includes(window.pid));
    const workspaces = new Set(matches.map(window => window.workspace));
    const workspace = [...workspaces][0];
    const result = workspaces.size === 1 && Number.isInteger(workspace) && workspace > 0 ? workspace : 0;
    return result;
};

/** Attach the Harbor indicator beside the current agent taskbar.
 * Parameters: panel - GNOME panel owning status indicators.
 * Returns: cleanup callback for every actor and timer.
 */
export function attachHarborStatus(panel) {
    print(`panel=${panel}`);
    const button = new PanelMenu.Button(0.5, 'Harbor runs', true);
    const box = new St.BoxLayout({style_class: 'ams-box panel-status-menu-box',
        y_expand: true, y_align: Clutter.ActorAlign.FILL});
    button.add_child(box);
    panel.addToStatusArea('harbor-runs', button, 0, 'right');
    const refresh = () => {
        const windows = global.get_window_actors().filter(actor => actor.meta_window).map(actor => {
            const window = actor.meta_window;
            return {pid: window.get_pid(), workspace: window.is_on_all_workspaces() ? 0 : (window.get_workspace()?.index() ?? -1) + 1};
        });
        renderHarborJobs(box, readHarborJobs(), windows);
        button.visible = box.visible;
        return GLib.SOURCE_CONTINUE;
    };
    refresh();
    const timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 2, refresh);
    const result = () => { GLib.source_remove(timer); button.destroy(); };
    print(result);
    return result;
}

/** Format a historical estimate without inventing a countdown when history is missing.
 * Parameters: job - normalized Harbor run and historical estimate.
 * Returns: approximate remaining minutes, overdue, unknown, or terminal status.
 */
export const formatHarborEstimate = (job) => {
    let result = job.status === 'running' ? '~?' : job.status;
    const seconds = job.estimate?.remaining_seconds;
    if (job.status === 'running' && Number.isFinite(seconds))
        result = seconds > 0 ? `~${Math.max(1, Math.ceil(seconds / 60))}m` : '~overdue';
    return result;
};
