import {DEFAULTS,simulate,epochTrace} from './lease-model.mjs';
const $=id=>document.getElementById(id);
const fields=Object.keys(DEFAULTS), units={kvMiB:' MiB',bandwidth:' GB/s',reuse:'×',contention:'%',overhead:' μs'};
let result, trace=[];
const reduced=matchMedia('(prefers-reduced-motion: reduce)');
let paused=reduced.matches;
const tiles=Array.from({length:16},(_,i)=>`<div class="tile ${[3,7,11,15].includes(i)?'b':''}" data-tile="${i}">${['CPU','NPU','GPU','ASIC'][i%4]}<small>TILE ${String(i).padStart(2,'0')}</small></div>`).join('');
$('chip-grid').innerHTML=tiles;
function size(n){return n<1024?`${n} B`:n<1048576?`${(n/1024).toFixed(1)} KiB`:`${(n/1048576).toFixed(1)} MiB`;}
function update(){
 const input=Object.fromEntries(fields.map(k=>[k,Number($(k).value)]));
 fields.forEach(k=>{$(`${k}-value`).textContent=input[k]+units[k]});
 result=simulate(input);
 $('speedup').textContent=result.speedup.toFixed(2)+'×';
 const selected=result.options.find(o=>o.id===result.selected);
 $('decision').textContent=selected.label+'. '+(result.selected==='copy'?'The assumed wait or lease cost outweighs locality.':'The assumed local wait costs less than copying.');
 $('route-label').textContent=selected.label;
 $('traffic').textContent=size(selected.bytes);
 const max=Math.max(...result.options.map(o=>o.us));
 $('comparison').innerHTML=result.options.map(o=>`<div class="bar-row ${o.id===result.selected?'chosen':''}"><div class="bar-label"><span>${o.id==='copy'?'Copy':o.id==='local'?'Local lease':'Remote lease'}${o.id===result.selected?' ↗':''}</span><strong>${(o.us/1000).toFixed(3)} ms</strong></div><div class="bar-track"><div style="width:${100*o.us/max}%"></div></div></div>`).join('');
 document.querySelectorAll('.tile').forEach((el,i)=>{el.classList.toggle('a',(result.selected==='local'?[8,9,12,13]:[0,1,4,5]).includes(i));el.classList.toggle('running',el.classList.contains('a'));});
}
fields.forEach(k=>$(k).addEventListener('input',()=>{document.querySelectorAll('[data-preset]').forEach(b=>{b.classList.remove('selected');b.setAttribute('aria-pressed','false')});update()}));
const presets={local:DEFAULTS,busy:{...DEFAULTS,kvMiB:64,contention:90},small:{...DEFAULTS,kvMiB:1,contention:0}};
function preset(key){fields.forEach(k=>{$(k).value=presets[key][k]});document.querySelectorAll('[data-preset]').forEach(b=>{b.classList.toggle('selected',b.dataset.preset===key);b.setAttribute('aria-pressed',String(b.dataset.preset===key))});update()}
document.querySelectorAll('[data-preset]').forEach(b=>b.addEventListener('click',()=>preset(b.dataset.preset)));
$('reset').addEventListener('click',()=>{preset('local');trace=[];$('epoch').textContent='EPOCH 07';$('trace-state').textContent='READY';$('trace').innerHTML='<li><span>00</span> Tenant A / epoch 7 / read-only</li><li><span>01</span> Tenant B / epoch 3 / independent</li><li><span>02</span> Waiting for a revocation experiment…</li>'});
function motion(){document.body.classList.toggle('paused',paused);$('motion').textContent=paused?'Resume motion ▷':'Pause motion Ⅱ';$('motion').setAttribute('aria-pressed',String(paused))}
$('motion').addEventListener('click',()=>{paused=!paused;motion()});
$('inject').addEventListener('click',()=>{trace=epochTrace();$('epoch').textContent='EPOCH 08';$('trace-state').textContent='STALE LEASE REFUSED';$('trace').innerHTML=trace.map((t,i)=>`<li class="${t.ok?'':'refused'}"><span>0${i}</span>${t.event}<br><strong>${t.ok?'✓':'×'} ${t.reason}</strong></li>`).join('');$('trace').setAttribute('aria-live','polite')});
$('export').addEventListener('click',()=>{const blob=new Blob([JSON.stringify({...result,trace,disclaimer:'Illustrative analytical model, not hardware or kernel execution.'},null,2)],{type:'application/json'});const url=URL.createObjectURL(blob);const a=document.createElement('a');a.href=url;a.download='aether-leaselab-scenario.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000)});
motion();update();
