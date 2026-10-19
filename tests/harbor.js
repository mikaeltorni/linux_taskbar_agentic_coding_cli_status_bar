// Exercise the actual Harbor taskbar renderer without a desktop session.
const Gio = imports.gi.Gio;
const [, bytes] = Gio.File.new_for_path(`${ARGV[0]}/acc-model-status@local/harbor.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;$/gm, '').replaceAll('export function', 'function');
const Clutter = {ActorAlign: {CENTER: 1}};
const St = {Label: class {
    /** Preserve requested label properties.
     * Parameters: this - mock label; options - actor properties.
     * Returns: undefined.
     */
    constructor(options) {
        print(`this=${this} options=${JSON.stringify(options)}`);
        Object.assign(this, options);
        print(undefined);
    }
}};
const api = new Function('St', 'Clutter', `${source}; return {renderHarborJobs};`)(St, Clutter);
const box = {children: [], destroy_all_children: () => { box.children = []; }, add_child: row => box.children.push(row)};
api.renderHarborJobs(box, [{name: 'demo', status: 'running'}]);
if (!box.visible || box.children[0].text !== 'Harbor demo running')
    throw Error('Running Harbor jobs must render on the panel');
api.renderHarborJobs(box, []);
if (box.visible || box.children.length)
    throw Error('No jobs must leave no empty indicator');
print('Harbor rendering checks passed');
