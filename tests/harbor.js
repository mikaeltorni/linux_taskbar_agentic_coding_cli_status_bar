// Exercise Harbor's shared-panel adaptation and speech placement without a desktop.
const Gio = imports.gi.Gio;
const [, bytes] = Gio.File.new_for_path(`${ARGV[0]}/acc-model-status@local/harbor.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;$/gm, '').replaceAll('export function', 'function').replaceAll('export const', 'const');
const traces = [];
const api = new Function('Gio', 'print', `${source}; return {harborPanelEntries, harborCountText, formatHarborEstimate, workspaceForHarbor, publishHarborPlacements};`)(Gio, (...values) => traces.push(values));
const job = {id: 'demo', name: 'demo', status: 'running', completed: 1, total: 4, fraction: 0.25, owner_pids: [42]};
const windows = [{pid: 42, workspace: 5, monitor: 1, focused: true}];
const rows = api.harborPanelEntries([job], windows);
if (rows.length !== 1 || rows[0].agent.workspace !== 5 || rows[0].agent.monitor !== 1 || !rows[0].agent.focused)
    throw Error('Harbor must join its actual terminal workspace/monitor');
if (rows[0].agent.workflow.fraction !== 0.25 || rows[0].agent.note_id)
    throw Error('Shared panel entries must preserve counts without pretending to be interactive agent notes');
if (api.harborCountText(job) !== '1/4' || api.harborCountText({...job, completed: 5}) !== '?/?')
    throw Error('Upper count must retain measured instance completion and reject inconsistent values');
const finished = api.harborPanelEntries([{...job, completed: 4, status: 'complete', finished_at: '2026-10-20T00:00:00Z'}], windows)[0].agent;
if (finished.phase !== 'DONE' || finished.run_active || finished.unread_completion)
    throw Error('Completed Harbor jobs must finish without inventing unread agent turns');
if (api.harborPanelEntries([{...job, status: 'interrupted'}], windows)[0].agent.phase !== 'INTERRUPTED')
    throw Error('Interrupted Harbor state must use the shared interrupted styling');
if (api.workspaceForHarbor(job, [{pid: 42, workspace: 3}, {pid: 42, workspace: 5}]) !== 0)
    throw Error('Ambiguous terminals must not borrow the focused workspace');
if (api.harborPanelEntries([], windows).length !== 0)
    throw Error('No Harbor jobs must leave no panel entries');
if (api.formatHarborEstimate({status: 'running', estimate: {remaining_seconds: 301}}) !== '~6m' ||
    api.formatHarborEstimate({status: 'running', estimate: {remaining_seconds: 0}}) !== '~1m' ||
    api.formatHarborEstimate({status: 'running'}) !== '~1m')
    throw Error('Forecasts must show approximate minutes without overdue text');
for (const seconds of [-30, NaN, Infinity, null]) {
    if (api.formatHarborEstimate({status: 'running', estimate: {remaining_seconds: seconds}}) !== '~1m')
        throw Error(`Forecast must remain a positive provisional estimate for ${seconds}`);
}
for (const status of ['complete', 'interrupted']) {
    if (api.formatHarborEstimate({status}) !== status)
        throw Error(`Terminal state must remain ${status}`);
}
let placement;
api.publishHarborPlacements({replace_contents: bytes => { placement = JSON.parse(new TextDecoder().decode(bytes)); }},
    [job], [{pid: 42, workspace: 7}]);
if (placement.jobs.demo !== 7 || !Number.isFinite(Date.parse(placement.updated_at)))
    throw Error('Speech handoff must retain the actual one-based workspace and fresh timestamp');
if (traces.length !== 0) throw Error('Routine Harbor adaptation must stay silent in the Shell journal');
print('Harbor shared-panel and speech placement checks passed');

// The public attach path feeds the existing owner and releases its polling timer.
let nativeWorkspace = 4;
let nativeJobs = [job];
let refreshes = 0;
let timerCallback;
let timerRemoved = false;
let publishedPlacement;
const window = {get_pid: () => 42, get_monitor: () => 1, is_on_all_workspaces: () => false,
    get_workspace: () => ({index: () => nativeWorkspace})};
const files = {new_for_path: path => ({
    get_parent: () => ({make_directory_with_parents: () => {}}),
    load_contents: () => [true, new TextEncoder().encode(JSON.stringify({updated_at: new Date().toISOString(), jobs: nativeJobs}))],
    replace_contents: bytes => { publishedPlacement = JSON.parse(new TextDecoder().decode(bytes)); },
})};
const attach = new Function('Gio', 'GLib', 'global', 'print', `${source}; return attachHarborStatus;`)(
    {File: files, FileCreateFlags: Gio.FileCreateFlags},
    {get_user_state_dir: () => '/sandbox', build_filenamev: parts => parts.join('/'), PRIORITY_DEFAULT: 0,
     SOURCE_CONTINUE: true, timeout_add_seconds: (_priority, seconds, callback) => { timerCallback = callback; return 7; },
     source_remove: id => { timerRemoved = id === 7; }},
    {get_window_actors: () => [{meta_window: window}], display: {focus_window: window}}, () => {});
const owner = {_queueRefresh: () => { refreshes++; }};
const cleanup = attach(owner);
if (owner._harborEntries[0].agent.workspace !== 5 || !refreshes || publishedPlacement.jobs.demo !== 5)
    throw Error('Attach must feed the common owner and preserve speech placement without creating another panel button');
nativeWorkspace = 1;
timerCallback();
if (owner._harborEntries[0].agent.workspace !== 2 || publishedPlacement.jobs.demo !== 2)
    throw Error('The attached observer must update actual workspace membership after terminal movement');
nativeJobs = [];
timerCallback();
if (owner._harborEntries.length !== 0) throw Error('Finished Harbor membership must clear from the common owner');
cleanup();
if (!timerRemoved || owner._harborEntries.length !== 0) throw Error('Detach must release the timer and its entries');
print('Harbor common-owner lifecycle checks passed');
