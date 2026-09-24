// Deterministic synthetic vectors for demonstrating views, never model output.
import { format } from './format.mjs';
import { writeFile } from 'node:fs/promises';
const topics = ['Account access', 'Billing', 'Integrations', 'Performance', 'Data export'];
const issues = [
  ['Password reset link expired','Cannot sign in after changing email','Two-factor code rejected','Invite never arrived','Session ends unexpectedly','Need to transfer account ownership','Locked out after failed attempts','Single sign-on redirect loops'],
  ['Invoice has the wrong address','Charged twice for the same month','Upgrade did not change the plan','Need a receipt for accounting','Refund for unused seats','Card update keeps failing','Tax number missing on invoice','Cancel renewal for this workspace'],
  ['Webhook events arrive twice','API token stopped working','Slack notifications are missing','Calendar sync skips meetings','OAuth connection disconnected','CRM fields do not match','Rate limit during nightly sync','Need to retry a failed integration'],
  ['Dashboard takes a long time to load','Search slows down on large projects','Import freezes halfway through','High memory usage in browser','Reports time out each morning','Scrolling stutters on long lists','Uploads are slower than yesterday','Query latency increases with filters'],
  ['CSV export contains duplicate rows','Downloaded archive is incomplete','Need JSON instead of CSV','Export dates use the wrong timezone','Attachments missing from backup','Restore a deleted record','Schedule a nightly export','Unicode text is garbled in spreadsheet'],
];
let seed=0x5e6a1234;
const random=()=>{seed^=seed<<13;seed^=seed>>>17;seed^=seed<<5;return(seed>>>0)/4294967296;};
const lines=['// SYNTHETIC vectors, generated deterministically; NOT embeddings from a model.', '// Regenerate: node browser/scripts/tickets-sample.mjs', 'schema {', '  type Ticket { title: String topic: String status: String embedding: Vector<16> related -> Ticket[] }', '  display { vector2d { Ticket }: Default vector3d { Ticket } table }', '}'];
for(let i=0;i<200;i++){
  const topic=Math.floor(i/40), v=Array.from({length:16},(_,d)=>(d===topic?1.8:0)+(random()-.5)*.7);
  const values=v.map(x=>x.toFixed(6)).join(', ');
  lines.push(`mutation { Ticket(title: ${JSON.stringify(issues[topic][i%8]+' #'+(i+1))} && topic: ${JSON.stringify(topics[topic])} && status: ${JSON.stringify(i%3?'Open':'Resolved')} && embedding: @vector[${values}]) { @id } }`);
}
// Most links join the same topic; selected cross-topic links expose disagreement.
for(let i=1;i<=200;i++)if(i%4===0){const next=i%20===0?(i+47)%200+1:(Math.floor((i-1)/40)*40+i%40+1);lines.push(`mutation { Ticket(@id: ${i}) { related -> link Ticket(@id: ${next}) { @id } } }`);}
lines.push('query { Ticket { @id title topic status embedding related -> Ticket { @id title } } }','');
await writeFile(new URL('../samples/tickets.zql',import.meta.url),format(lines.join('\n')));
