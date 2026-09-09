(function () {
'use strict';
const clamp = (v, min=0, max=1) => Math.min(max, Math.max(min,v));
const smooth = v => {const x=clamp(v);return x*x*(3-2*x);};
function raceState(progress) {
 const p=clamp(Number.isFinite(progress)?progress:0);
 return {p,lane:smooth((p-.12)/.25)*(1-smooth((p-.82)/.18)),overlap:clamp(1-Math.abs(p-.52)/.22),complete:p>=.72,chapter:p<.32?0:p<.72?1:2,gap:p>=.72?((p-.72)*2.5+.1).toFixed(1):Math.max(0,.8-p*1.25).toFixed(1)};
}
if(typeof module!=='undefined' && module.exports) module.exports={raceState,clamp};
if(typeof document==='undefined')return;
document.documentElement.classList.remove('no-js');
const section=document.querySelector('.pass-scroll'),stage=document.querySelector('.race-stage');
const chapters=[...document.querySelectorAll('[data-chapter]')], motionButton=document.getElementById('motion-toggle');
const reduced=window.matchMedia('(prefers-reduced-motion: reduce)');
let paused=reduced.matches,lastChapter=-1,lastComplete=null,queued=false;
function renderRace(progress){
 if(!stage)return;
 const s=raceState(progress);
 stage.style.setProperty('--pass',s.p.toFixed(4));stage.style.setProperty('--lane',s.lane.toFixed(4));stage.style.setProperty('--overlap',s.overlap.toFixed(4));stage.classList.toggle('pass-complete',s.complete);
 document.getElementById('rival-gap').textContent=s.gap;
 if(lastChapter!==s.chapter){chapters.forEach((c,i)=>c.classList.toggle('active',i===s.chapter));document.getElementById('pass-status').textContent=['Closing the gap','Alongside. Hold your line.','Pass complete. Position gained.'][s.chapter];lastChapter=s.chapter;}
 if(lastComplete!==s.complete){document.querySelector('.rival-position').textContent=s.complete?'8':'7';document.getElementById('position-message').textContent=s.complete?'P7. One position gained.':'A place worth chasing.';lastComplete=s.complete;}
}
function update(){
 queued=false;if(!section||paused||reduced.matches)return;
 const bounds=section.getBoundingClientRect();if(bounds.bottom<0||bounds.top>window.innerHeight)return;
 const sticky=section.querySelector('.pass-sticky'), top=parseFloat(getComputedStyle(sticky).top)||100;
 renderRace((top-bounds.top)/Math.max(1,section.offsetHeight-sticky.offsetHeight));
}
function schedule(){if(!queued){queued=true;requestAnimationFrame(update);}}
function setMotionLabel(){if(!motionButton)return;motionButton.setAttribute('aria-pressed',String(paused));motionButton.textContent=paused?'Resume motion ?':'Pause motion ?';motionButton.disabled=reduced.matches;if(reduced.matches)motionButton.textContent='Reduced motion on';}
if(section){
 renderRace(reduced.matches?.52:0);setMotionLabel();
 window.addEventListener('scroll',schedule,{passive:true});window.addEventListener('resize',schedule,{passive:true});window.addEventListener('load',schedule,{once:true});
 motionButton?.addEventListener('click',()=>{paused=!paused;document.body.classList.toggle('motion-paused',paused);setMotionLabel();if(!paused)schedule();});
 reduced.addEventListener('change',()=>{paused=reduced.matches;setMotionLabel();if(reduced.matches)renderRace(.52);else schedule();});schedule();
}
const tabs=[...document.querySelectorAll('[data-widget]')];
function activateTab(tab,focus=false){tabs.forEach(item=>{const selected=item===tab;item.setAttribute('aria-selected',String(selected));item.tabIndex=selected?0:-1;const panel=document.getElementById(item.getAttribute('aria-controls'));if(panel){panel.hidden=!selected;panel.classList.toggle('active',selected);}});if(focus)tab.focus({preventScroll:true});}
tabs.forEach((tab,index)=>{tab.addEventListener('click',()=>activateTab(tab));tab.addEventListener('keydown',event=>{let next;if(event.key==='ArrowRight')next=(index+1)%tabs.length;if(event.key==='ArrowLeft')next=(index-1+tabs.length)%tabs.length;if(event.key==='Home')next=0;if(event.key==='End')next=tabs.length-1;if(next!==undefined){event.preventDefault();activateTab(tabs[next],true);}});});
})();
