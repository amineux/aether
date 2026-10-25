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
test('malformed grants cannot supply authority or bypass the byte bound',()=>{
 for(const grant of [null,undefined,[],{},
  {...g,bytes:NaN},{...g,bytes:Infinity},{...g,bytes:'128'},
  {...g,bytes:0},{...g,bytes:-1},{...g,bytes:1.5},
  {...g,bytes:Number.MAX_SAFE_INTEGER+1},
  {...g,tenant:''},{...g,tenant:' '},{...g,tenant:undefined},
  {...g,epoch:undefined},{...g,epoch:-1},{...g,epoch:NaN},
  {...g,epoch:1.5},{...g,epoch:Number.MAX_SAFE_INTEGER+1},
  {...g,write:'false'},{...g,write:1},{...g,write:undefined}]) {
  assert.deepEqual(authorize(grant,q,7),{ok:false,reason:'malformed grant'});
 }
});
test('missing identities and epochs cannot authorize each other',()=>{
 assert(!authorize({...g,tenant:undefined},{...q,tenant:undefined},7).ok);
 assert(!authorize({...g,epoch:undefined},q,undefined).ok);
});
test('malformed requests refuse without throwing or coercing permissions',()=>{
 for(const request of [null,undefined,[],{},
  {...q,tenant:''},{...q,tenant:5},{...q,write:undefined},
  {...q,write:0},{...q,write:'false'}]) {
  assert.deepEqual(authorize(g,request,7),{ok:false,reason:'malformed request'});
 }
 for(const write of ['true','false',0,1]) {
  assert(!authorize({...g,write}, {...q,write:true},7).ok);
 }
});
test('current epochs must be nonnegative exact integers',()=>{
 for(const epoch of [undefined,null,'7',NaN,Infinity,-1,1.5,Number.MAX_SAFE_INTEGER+1]) {
  assert.deepEqual(authorize(g,q,epoch),{ok:false,reason:'invalid current epoch'});
 }
 assert(authorize({...g,epoch:0},q,0).ok);
 assert(authorize({...g,epoch:Number.MAX_SAFE_INTEGER},q,Number.MAX_SAFE_INTEGER).ok);
});
test('safe-integer range endpoints admit exact ends and refuse overflow',()=>{
 const grant={...g,bytes:Number.MAX_SAFE_INTEGER};
 assert(authorize(grant,{...q,offset:grant.bytes-1,length:1},7).ok);
 assert(authorize(grant,{...q,offset:0,length:grant.bytes},7).ok);
 for(const request of [
  {...q,offset:grant.bytes,length:1},
  {...q,offset:grant.bytes-1,length:2},
  {...q,offset:Number.MAX_SAFE_INTEGER+1,length:1},
  {...q,offset:0,length:Number.MAX_SAFE_INTEGER+1},
  {...q,offset:'64'},{...q,length:'64'},
  {...q,offset:0.5},{...q,length:0.5},
 ]) assert.deepEqual(authorize(grant,request,7),{ok:false,reason:'outside lease range'});
});
test('small-range exhaustive check matches an independent exact-integer oracle',()=>{
 for(let bytes=1;bytes<=16;bytes++) for(let offset=-1;offset<=18;offset++) for(let length=-1;length<=18;length++) {
  const expected=offset>=0&&length>0&&BigInt(offset)+BigInt(length)<=BigInt(bytes);
  assert.equal(authorize({...g,bytes},{...q,offset,length},7).ok,expected);
 }
 assert(authorize({...g,write:true},{...q,write:true},7).ok);
});
