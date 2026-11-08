// Verify question badges independently of the retained task's progress.
const Gio = imports.gi.Gio;
const root = ARGV[0] || '.';
const [, bytes] = Gio.File.new_for_path(`${root}/acc-model-status@local/extension.js`).load_contents(null);
const source = new TextDecoder().decode(bytes).replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const api = new Function('GObject','PanelMenu','Extension',`${source}\nreturn {agentPhaseBadgeText,agentPhaseBadgeColor,panelTaskProgressValue};`)(
    {registerClass: value => value},{Button:class {}},class {});
const row = {phase:'DONE',ready_completed_at:'2026-11-07T12:00:00Z',unread_completion:true,
    workflow:{available:true,fraction:.4,task_complete:false,
        interaction:{mode:'question',state:'complete',label:'Question answered'}}};
if (api.agentPhaseBadgeText(row) !== 'ANSWERED') throw Error('question response must have its own badge');
if (api.agentPhaseBadgeColor(row) !== '#5ca8ff') throw Error('question badge must be blue');
if (api.panelTaskProgressValue(row) !== .4) throw Error('answer completion must retain unfinished task fraction');
row.workflow.interaction.state='active';
if (api.agentPhaseBadgeText(row) !== 'QUESTION') throw Error('active question badge missing');
print('PASS: questions retain task progress and distinct active/answered badges');
