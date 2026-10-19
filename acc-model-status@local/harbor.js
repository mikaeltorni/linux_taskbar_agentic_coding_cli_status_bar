/* Independent Harbor status, published by harbor-taskbar.service. */
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

/** Read the independent watcher's atomically published snapshot.
 * Parameters: none.
 * Returns: current jobs, or an empty list for absent, malformed or stale data.
 */
export const readHarborJobs = () => {
    let result = [];
    try {
        const [ok, bytes] = harborStateFile('status.json').load_contents(null);
        const data = ok ? JSON.parse(new TextDecoder().decode(bytes)) : null;
        if (Array.isArray(data?.jobs) && Date.now() - Date.parse(data.updated_at) < 30000)
            result = data.jobs;
    } catch (_error) {
        // Startup and atomic replacement races are normal.
    }
    return result;
};

/** Adapt independent Harbor runs to the common workspace panel renderer.
 * Parameters: jobs - normalized Harbor jobs; windows - actual terminal window descriptors.
 * Returns: panel-only task/provider pairs with measured progress and original Harbor counts.
 */
export const harborPanelEntries = (jobs, windows) => jobs.map(job => {
    const workspace = workspaceForHarbor(job, windows);
    const matches = matchingHarborWindows(job, windows).filter(window => window.workspace === workspace);
    const monitors = new Set(matches.map(window => window.monitor));
    const key = `harbor:${job.id}`;
    const phase = {running: 'RUNNING', complete: 'DONE', interrupted: 'INTERRUPTED'}[job.status] || 'UNKNOWN';
    const fraction = harborProgress(job);
    return {key, provider: {kind: 'harbor', label: 'Harbor'}, agent: {
        key, session_id: key, note_id: '', title: `Harbor ${job.name}`, workspace,
        monitor: monitors.size === 1 ? [...monitors][0] : null,
        focused: matches.some(window => window.focused === true),
        phase, run_active: phase === 'RUNNING', unread_completion: false,
        ready_completed_at: phase === 'DONE' ? job.finished_at : '',
        workflow: {available: fraction !== null, fraction}, harbor: job,
    }};
});

/** Format the measured instance count for the common cell's upper timing slot.
 * Parameters: job - normalized Harbor trial counts.
 * Returns: completed/total text or ?/? when progress is unavailable.
 */
export const harborCountText = job => harborProgress(job) === null ? '?/?' : `${job.completed}/${job.total}`;

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

/** Select only windows owned by the observed Harbor terminal/client ancestry.
 * Parameters: job - terminal owner pids; windows - actual Shell window descriptors.
 * Returns: windows matching that independent process identity.
 */
export const matchingHarborWindows = (job, windows) => windows.filter(window => job.owner_pids?.includes(window.pid));

/** Match actual owner windows without borrowing desktop focus or ambiguous clients.
 * Parameters: job - terminal owner pids; windows - actual one-based workspace descriptors.
 * Returns: unique workspace number or zero when unknown, sticky or ambiguous.
 */
export const workspaceForHarbor = (job, windows) => {
    const matches = matchingHarborWindows(job, windows);
    const workspaces = new Set(matches.map(window => window.workspace));
    const workspace = [...workspaces][0];
    const result = workspaces.size === 1 && Number.isInteger(workspace) && workspace > 0 ? workspace : 0;
    return result;
};

/** Feed independent Harbor tasks into the existing agent workspace groups.
 * Parameters: owner - existing common taskbar button.
 * Returns: cleanup callback for the independent refresh timer and panel entries.
 */
export function attachHarborStatus(owner) {
    print(`owner=${owner}`);
    const placementsFile = harborStateFile('placements.json');
    try { placementsFile.get_parent().make_directory_with_parents(null); } catch (_error) { /* Already exists. */ }
    const refresh = () => {
        const windows = global.get_window_actors().filter(actor => actor.meta_window).map(actor => {
            const window = actor.meta_window;
            return {pid: window.get_pid(), monitor: window.get_monitor(), focused: window === global.display.focus_window,
                workspace: window.is_on_all_workspaces() ? 0 : (window.get_workspace()?.index() ?? -1) + 1};
        });
        const jobs = readHarborJobs();
        owner._harborEntries = harborPanelEntries(jobs, windows);
        publishHarborPlacements(placementsFile, jobs, windows);
        owner._queueRefresh();
        return GLib.SOURCE_CONTINUE;
    };
    refresh();
    const timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 2, refresh);
    const result = () => { GLib.source_remove(timer); owner._harborEntries = []; };
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

/** Resolve one independent Harbor state document.
 * Parameters: name - observer-owned JSON filename.
 * Returns: the file beneath the user's XDG state home.
 */
export const harborStateFile = name => Gio.File.new_for_path(
    GLib.build_filenamev([GLib.get_user_state_dir(), 'harbor-taskbar', name]));

/** Publish actual window placement for headless completion speech.
 * Parameters: file - placement destination; jobs - current jobs; windows - live Shell windows.
 * Returns: undefined; unavailable files never break panel updates.
 */
export const publishHarborPlacements = (file, jobs, windows) => {
    const placements = Object.fromEntries(jobs.map(job => [job.id, workspaceForHarbor(job, windows)]));
    const data = {updated_at: new Date().toISOString(), jobs: placements};
    try {
        file.replace_contents(new TextEncoder().encode(JSON.stringify(data)), null, false,
            Gio.FileCreateFlags.PRIVATE | Gio.FileCreateFlags.REPLACE_DESTINATION, null);
    } catch (_error) { /* Read-only homes must not break the Shell. */ }
};
