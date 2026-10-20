import test from 'node:test';
import assert from 'node:assert/strict';
import {simulate,authorize,epochTrace} from '../site/js/lease-model.mjs';
test('binary payload and decimal bandwidth use consistent units',()=>{
 const r=simulate({kvMiB:1,bandwidth:1,reuse:1,contention:0,overhead:20});
 assert.equal(r.options[0].us,1048.576+200);
 assert.equal(r.options[1].us,220);
 assert.equal(r.options[2].bytes,1048576+32);
});
test('contention and setup can reverse the locality decision',()=>{
 assert.equal(simulate().selected,'local');
 assert.equal(simulate({kvMiB:64,contention:90}).selected,'copy');
 assert.equal(simulate({kvMiB:1,contention:0}).selected,'copy');
 assert.equal(simulate({kvMiB:1,overhead:500}).selected,'copy');
});
test('zero overhead and one remote pass tie with copying; copying wins stable tie',()=>{
 assert.equal(simulate({kvMiB:1,bandwidth:256,reuse:1,contention:95,overhead:0}).selected,'copy');
});
test('all slider extremes produce finite positive times and a true minimum',()=>{
 for(const kvMiB of [1,2048]) for(const bandwidth of [1,256]) for(const reuse of [1,16]) for(const contention of [0,95]) for(const overhead of [0,500]){
 const r=simulate({kvMiB,bandwidth,reuse,contention,overhead});
 assert(r.options.every(o=>Number.isFinite(o.us)&&o.us>0));
 assert.equal(r.options.find(o=>o.id===r.selected).us,Math.min(...r.options.map(o=>o.us)));
 assert(r.speedup>=1);assert(r.breakEvenContention>=0&&r.breakEvenContention<100);
 }
});
test('invalid parameters cannot produce plausible-looking output',()=>{
 for(const p of [{bandwidth:0},{reuse:NaN},{contention:100},{kvMiB:-1},{overhead:Infinity}])assert.throws(()=>simulate(p),RangeError);
});
const g={tenant:'A',epoch:7,bytes:128,write:false};const q={tenant:'A',offset:64,length:64,write:false};
test('lease accepts exact end boundary and refuses tenant, epoch, permission and range violations',()=>{
 assert(authorize(g,q,7).ok);
 for(const [request,epoch] of [[{...q,tenant:'B'},7],[q,8],[{...q,write:true},7],[{...q,length:65},7],[{...q,offset:-1},7],[{...q,length:0},7],[{...q,offset:NaN},7]])assert(!authorize(g,request,epoch).ok);
});
test('epoch replay is refused while independent neighbor remains live',()=>{
 const trace=epochTrace();assert(trace[0].ok);assert.equal(trace[2].reason,'stale epoch');assert(!trace[2].ok);assert(trace[3].ok);
});
