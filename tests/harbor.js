// Exercise the actual Harbor taskbar renderer without a desktop session.
const Gio = imports.gi.Gio;
const [, bytes] = Gio.File.new_for_path(`${ARGV[0]}/acc-model-status@local/harbor.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;$/gm, '').replaceAll('export function', 'function').replaceAll('export const', 'const');
const Clutter = {ActorAlign: {CENTER: 1, START: 2, END: 3, FILL: 4}, BinLayout: class {}};
class Actor {
    /** Preserve actor properties and children for actual renderer execution.
     * Parameters: this - mock actor; options - construction properties.
     * Returns: undefined.
     */
    constructor(options = {}) {
        print(`this=${this} options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        this.children = [];
        print(undefined);
    }
    add_child = row => { row.parent = this; this.children.push(row); };
    destroy_all_children = () => { this.children = []; };
    destroy = () => { this.parent.children = this.parent.children.filter(child => child !== this); };
    connect = (signal, callback) => { this[signal] = callback; };
    queue_repaint = () => { this.repaints = (this.repaints || 0) + 1; };
}
const St = {Label: Actor, BoxLayout: Actor, Widget: Actor, DrawingArea: Actor};
const traces = [];
const api = new Function('St', 'Clutter', 'print', `${source}; return {renderHarborJobs, formatHarborEstimate, paintHarborBar, workspaceForHarbor};`)(St, Clutter, (...values) => traces.push(values));
const box = new Actor();
const job = {id: 'demo', name: 'demo', status: 'running', completed: 1, total: 4, fraction: 0.25, owner_pids: [42]};
api.renderHarborJobs(box, [job], [{pid: 42, workspace: 5}]);
const group = box.children[0];
const cell = group.children[1];
const [count, bar, eta] = cell.children;
if (!box.visible || count.text !== '1/4' || !count.style.includes('#f472b6') || count.y_align !== Clutter.ActorAlign.START)
    throw Error('Pink instance count must replace the upper timer');
if (eta.text !== '~?' || eta.y_align !== Clutter.ActorAlign.END || bar.text)
    throw Error('ETA must remain below the bar; no text inside the fill');
if (group.children[0].text !== 'H W5')
    throw Error('Workspace must come from the actual owner window');
const rectangles = [];
const area = {get_surface_size: () => [52, 8], get_context: () => ({
    setSourceRGBA: () => {}, rectangle: (...args) => rectangles.push(args), fill: () => {}, $dispose: () => {},
})};
api.paintHarborBar(area, job);
if (rectangles[1][2] !== 13)
    throw Error('One of four trials must fill one quarter of the bar');
api.renderHarborJobs(box, [{...job, completed: 3, fraction: 0.75, estimate: {remaining_seconds: 301}}], [{pid: 42, workspace: 5}]);
if (box.children[0] !== group || count.text !== '3/4' || eta.text !== '~6m')
    throw Error('Retain actors while updating progress and ETA');
rectangles.length = 0;
api.paintHarborBar(area, {...job, completed: 3, fraction: 0.75});
if (rectangles[1][2] !== 39)
    throw Error('Progress fill must update with completed instances');
if (api.workspaceForHarbor(job, [{pid: 42, workspace: 3}, {pid: 42, workspace: 5}]) !== 0)
    throw Error('Ambiguous terminals must not borrow the focused workspace');
api.renderHarborJobs(box, []);
if (box.visible || box.children.length)
    throw Error('No jobs must leave no empty indicator');
if (api.formatHarborEstimate({status: 'running', estimate: {remaining_seconds: 0}}) !== '~overdue' ||
    api.formatHarborEstimate({status: 'running'}) !== '~?')
    throw Error('Forecasts must preserve overdue and unknown states');
print('Harbor rendering checks passed');

if (traces.length !== 0) throw Error('Routine Harbor rendering must stay silent in the Shell journal');
