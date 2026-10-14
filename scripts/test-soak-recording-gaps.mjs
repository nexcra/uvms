import assert from 'node:assert/strict';
import {unexplainedGaps} from './soak-recording-gaps.mjs';

// Actual fixture: one completed 15-second file was absent from the latest-file poll.
const clips=[
  {file:'first',start_ms:1791999714835,end_ms:1791999729835},
  {file:'middle',start_ms:1791999729793,end_ms:1791999744793},
  {file:'last',start_ms:1791999745838,end_ms:1791999750771},
];
assert.equal(unexplainedGaps([clips[0],clips[2]],'pcmu',[]).length,1);
assert.deepEqual(unexplainedGaps(clips,'pcmu',[]),[]);
const missing=[clips[0],clips[2]];
const outage={done:true,affected:['pcmu'],started:1791999730000,restored:1791999740000};
assert.deepEqual(unexplainedGaps(missing,'pcmu',[outage]),[]);
assert.equal(unexplainedGaps(missing,'other',[outage]).length,1);
console.log('Recording gap regression passed');
