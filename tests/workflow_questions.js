// Verify question states use colors while retaining the underlying task display.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const api = new Function('GObject','PanelMenu','Extension',`${source}\nreturn {agentPhaseBadgeText,agentPhaseBadgeColor,panelTaskProgressValue,panelAgentEstimateText,panelAgentRuntimeColor,panelProgressColor};`)(
    {registerClass: value => value},{Button:class {}},class {});
const row = {phase:'DONE',ready_completed_at:'2026-11-07T12:00:00Z',unread_completion:true,
    workflow:{available:true,fraction:.4,task_complete:false,
        interaction:{mode:'question',state:'complete',label:'Question answered'}}};
if (api.agentPhaseBadgeText(row) !== 'DONE') throw Error('answer must retain the task badge');
if (api.agentPhaseBadgeColor(row) !== '#5ca8ff') throw Error('answer badge must be blue');
if (api.panelAgentRuntimeColor(row, 0) !== '#5ca8ff') throw Error('answer runtime must be blue');
if (api.panelProgressColor(.4, false, 0, row).join(',') !== '92,168,255') throw Error('answer bar must be blue');
if (api.panelAgentEstimateText(row) !== '') throw Error('answer must not add a lower label');
if (api.panelTaskProgressValue(row) !== .4) throw Error('answer must retain unfinished task fraction');
row.workflow.interaction.state='active';
row.phase='RUNNING'; row.run_active=true;
if (api.agentPhaseBadgeText(row) !== 'RUNNING') throw Error('question must retain the task badge');
if (api.agentPhaseBadgeColor(row) !== '#c084fc') throw Error('question badge must be violet');
if (api.panelAgentRuntimeColor(row, 0) !== '#c084fc') throw Error('question runtime must be violet');
if (api.panelProgressColor(.4, false, 0, row).join(',') !== '192,132,252') throw Error('question bar must be violet');
if (api.panelAgentEstimateText(row) !== '') throw Error('question must not add a lower label');
print('PASS: question states use distinct colors and retain task display');
