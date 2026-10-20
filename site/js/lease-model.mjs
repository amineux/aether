/** LeaseLab v1: deterministic, analytical proposal. Not a kernel or hardware benchmark. */
export const DEFAULTS = Object.freeze({kvMiB:256, bandwidth:64, reuse:4, contention:25, overhead:20});
export function simulate(input = {}) {
  const p = {...DEFAULTS, ...input};
  for (const [k, lo, hi] of [['kvMiB',1,2048],['bandwidth',1,256],['reuse',1,16],['contention',0,95],['overhead',0,500]]) {
    if (!Number.isFinite(p[k]) || p[k]<lo || p[k]>hi) throw new RangeError(`Invalid ${k}`);
  }
  // Binary data size, decimal GB/s. 1 GB/s = 1000 bytes/us.
  const bytes = p.kvMiB * 1048576;
  const transferUs = bytes / (p.bandwidth * 1000);
  const computeUs = 200 * p.reuse; // Explicit scenario assumption, not measured.
  const waitUs = computeUs * p.contention / (100 - p.contention);
  const copy = {id:'copy', label:'Copy to a free tile', us:transferUs+computeUs, bytes, waitUs:0};
  const local = {id:'local', label:'Lease at the data', us:p.overhead+waitUs+computeUs, bytes:32, waitUs};
  const remote = {id:'remote', label:'Lease across the fabric', us:p.overhead+transferUs*p.reuse+computeUs, bytes:32+bytes*p.reuse, waitUs:0};
  const options = [copy, local, remote];
  const selected = options.reduce((a,b)=>b.us<a.us?b:a); // Stable ties favor copying.
  return {version:'leaselab-v1', parameters:p, options, selected:selected.id,
    speedup:copy.us/selected.us, savedBytes:copy.bytes-selected.bytes,
    breakEvenContention:100*Math.max(0,transferUs-p.overhead)/(computeUs+Math.max(0,transferUs-p.overhead)),
    assumptions:{computeUsPerPass:200, grantBytes:32, bandwidthUnits:'decimal GB/s', sizeUnits:'MiB', serial:true, localDataTrafficExcluded:true}};
}
// Protocol sketch: tenant + epoch + range + permission are all required.
export function authorize(grant, request, currentEpoch) {
  if(request.tenant!==grant.tenant) return {ok:false,reason:'wrong tenant'};
  if(grant.epoch!==currentEpoch) return {ok:false,reason:'stale epoch'};
  if(request.write && !grant.write) return {ok:false,reason:'read-only lease'};
  if(!Number.isSafeInteger(request.offset)||!Number.isSafeInteger(request.length)||request.offset<0||request.length<=0||request.offset+request.length>grant.bytes) return {ok:false,reason:'outside lease range'};
  return {ok:true,reason:'authorized'};
}
export function epochTrace(){
  const grant={tenant:'A',epoch:7,bytes:1048576,write:false};
  const request={tenant:'A',offset:0,length:64,write:false};
  return [
    {event:'Read under epoch 7',...authorize(grant,request,7)},
    {event:'Revoke: current epoch → 8',ok:true,reason:'new admissions require epoch 8'},
    {event:'Replay old lease',...authorize(grant,request,8)},
    {event:'Neighbor B reads its own lease',...authorize({...grant,tenant:'B',epoch:3},{...request,tenant:'B'},3)}
  ];
}
