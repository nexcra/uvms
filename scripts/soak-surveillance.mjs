// Actual four-camera UI in three browsers modes; only the isolated Rust soak fixture.
import {chromium} from 'playwright';
import {spawn, execFileSync} from 'node:child_process';
import {mkdir, writeFile, appendFile} from 'node:fs/promises';
import path from 'node:path';
import {unexplainedGaps} from './soak-recording-gaps.mjs';

const base='http://127.0.0.1:8099', aliases=['aac','pcmu','pcma','opus'];
const seconds=Number(process.env.UVMS_SOAK_SECONDS||3600);
if(!Number.isInteger(seconds)||seconds<480||seconds>86400)throw Error('Duration must be 480..86400 seconds');
const output=path.resolve(process.env.UVMS_SOAK_OUTPUT||'test-results/soak/final');
await mkdir(output,{recursive:true});
const linux='/mnt/d/workspaces/uvms/scripts/soak-fixture.py';
const action=(verb,camera)=>execFileSync('wsl',['-d','Ubuntu-24.04','-u','root','--','python3',linux,verb,...(camera?['--camera',camera]:[])],{encoding:'utf8',timeout:45000});
const api=async(route,body)=>{
  const r=await fetch(base+route,{signal:AbortSignal.timeout(10000),...(body?{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}:{})});
  if(!r.ok)throw Error(`${route}: HTTP ${r.status}`);return r.json();
};
const allClips=async()=>{
  const clips=[];
  for(let offset=0;offset<=1000000;offset+=1000){
    const page=(await api('/api/recordings?limit=1000&offset='+offset)).files;clips.push(...page);
    if(page.length<1000)return clips;
  }
  throw Error('Recording enumeration exceeded API pagination bound');
};
const report={seconds,startedAt:new Date().toISOString(),fixture:JSON.parse(action('info')),health:await api('/healthz'),events:[],failures:[],samples:[],metrics:[],mediaErrors:[]};
report.thresholds={sampleIntervalSeconds:5,audioWindowsPerSample:5,audioWindowMs:100,minimumSampleFps:5,persistentBadSampleSeconds:10,recoveryGraceSeconds:75,recordingFreshnessSeconds:45};
if(report.health.media_engine!=='rust-native')throw Error('Expected Rust test service');
const save=()=>writeFile(path.join(output,'result.json'),JSON.stringify(report,null,2));
let sampler;
function startSampler(){
  sampler=spawn('wsl',['-d','Ubuntu-24.04','-u','root','--','env','UVMS_BENCH_SERVICE=uvms-soak','UVMS_BENCH_SOURCE_PORT=18576','python3','-u','/mnt/d/workspaces/uvms/scripts/sample-service.py']);
  let pending='';
  sampler.stdout.on('data',chunk=>{pending+=chunk;let end;while((end=pending.indexOf('\n'))>=0){const line=pending.slice(0,end);pending=pending.slice(end+1);try{report.metrics.push(JSON.parse(line));}catch{}}});
  sampler.stderr.on('data',chunk=>report.events.push({kind:'sampler-stderr',message:String(chunk)}));
}
startSampler();
const browser=await chromium.launch({executablePath:process.env.UVMS_BROWSER||'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',headless:true,args:['--autoplay-policy=no-user-gesture-required','--disable-background-timer-throttling','--disable-renderer-backgrounding']});
const pages=[];
let stopRequested=false;
process.once('SIGINT',()=>{stopRequested=true;});
process.once('SIGTERM',()=>{stopRequested=true;});
const grace=new Map(aliases.map(a=>[a,0]));
const events=seconds<1000?[
  {at:40,kind:'source-off',camera:'aac',duration:30},
  {at:140,kind:'blackhole',camera:'pcmu',duration:40},
  {at:260,kind:'restart-inputs'}, {at:380,kind:'restart-gateway'}
]:[
  {at:120,kind:'source-off',camera:'aac',duration:30},
  {at:300,kind:'blackhole',camera:'pcmu',duration:40},
  {at:600,kind:'restart-inputs'}, {at:900,kind:'restart-gateway'},
  ...[1200,1440,1680,1920,2160].map(at=>({at,kind:'source-off',camera:'aac',duration:20}))
];
let start=0;
try{
  report.recordingControls={};
  for(const alias of aliases){
    const control=['pcma','opus'].includes(alias)?{mode:'auto',schedules:[{start_ms:Date.now(),end_ms:Date.now()+(seconds+180)*1000,repeat_seconds:null}]}:{mode:'on',schedules:[]};
    report.recordingControls[alias]=control;await api('/api/recording/'+alias,control);
  }
  for(const mode of ['webrtc','mse','hls']){
    const page=await browser.newPage({viewport:{width:1440,height:1100}});pages.push({mode,page});
    page.on('pageerror',e=>report.failures.push({kind:'pageerror',mode,message:e.message,elapsed:(Date.now()-start)/1000}));
    await page.addInitScript(()=>{
      window.soakErrors=[];
      document.addEventListener('error',e=>{if(e.target instanceof HTMLMediaElement)window.soakErrors.push({at:Date.now(),id:e.target.closest('[data-camera]')?.dataset.camera,code:e.target.error?.code,message:e.target.error?.message});},true);
    });
    await page.goto(base);
    await page.waitForSelector('#camera-choices input[value="opus"]',{state:'attached'});
    await page.locator('button[data-layout="4"]').click();
    for(const alias of aliases)await page.locator(`#camera-choices input[value="${alias}"]`).check();
    await page.locator('#mode').selectOption(mode);await page.locator('#connect').click();
    await page.waitForFunction(()=>document.querySelectorAll('.camera-tile[data-state="live"]').length===4,null,{timeout:65000});
    await page.evaluate(async()=>{
      window.soakAudio=new AudioContext();await soakAudio.resume();
      window.soakAnalysers=new Map();window.soakElementSources=new WeakMap();
      window.soakSnapshot=async()=>{
        // Multiple audio windows avoid declaring a transient decoder buffer gap a persistent outage.
        const players=[...wallPlayers.values()];
        for(const p of players){
        const video=p.video,track=video.srcObject?.getAudioTracks()[0];
        let node=soakAnalysers.get(p.id);
        if(!node||node.track!==track){
          node?.source.disconnect();node?.analyser.disconnect();node?.gain.disconnect();
          let source;
          if(track)source=soakAudio.createMediaStreamSource(new MediaStream([track]));
          else {source=soakElementSources.get(video);if(!source){source=soakAudio.createMediaElementSource(video);soakElementSources.set(video,source);}}
          const analyser=soakAudio.createAnalyser(),gain=soakAudio.createGain();gain.gain.value=0;
          source.connect(analyser);analyser.connect(gain);gain.connect(soakAudio.destination);
          node={source,analyser,gain,track,elementSource:track?null:source};soakAnalysers.set(p.id,node);
        }
        if(!video.srcObject)video.muted=false;
        node.maxRms=0;node.audibleWindows=0;
        }
        for(let i=0;i<5;i++){
          await new Promise(r=>setTimeout(r,100));
          for(const p of players){const node=soakAnalysers.get(p.id),values=new Float32Array(node.analyser.fftSize);node.analyser.getFloatTimeDomainData(values);
            const rms=Math.sqrt(values.reduce((sum,v)=>sum+v*v,0)/values.length);node.maxRms=Math.max(node.maxRms,rms);if(rms>0.0001)node.audibleWindows++;
          }
        }
        return players.map(p=>{const video=p.video,quality=video.getVideoPlaybackQuality(),node=soakAnalysers.get(p.id);
          return {id:p.id,generation:p.generation,state:p.state,frames:quality.totalVideoFrames-quality.droppedVideoFrames,
            time:video.currentTime,width:video.videoWidth,height:video.videoHeight,paused:video.paused,
            rms:node.maxRms,audibleWindows:node.audibleWindows,audioWindows:5,audioBytes:video.webkitAudioDecodedByteCount||0};
        });
      };
    });
  }
  await new Promise(r=>setTimeout(r,5000));start=Date.now();report.measurementStartedAt=new Date(start).toISOString();
  let previous=new Map(),badSince=new Map(),previousClips=new Map();
  while((Date.now()-start)/1000<seconds&&!stopRequested){
    const elapsed=(Date.now()-start)/1000;
    for(const e of events){
      if(!e.done&&elapsed>=e.at){
        e.done=true;e.started=Date.now();e.affected=e.camera?[e.camera]:aliases;
        for(const alias of e.affected)grace.set(alias,e.started+(e.duration||0)*1000+75000);
        if(e.kind==='restart-gateway'){
          report.beforeRestartClips=await allClips();
          sampler.kill();
        }
        action(e.kind,e.camera);e.actionFinished=Date.now();report.events.push({...e});
        if(e.kind==='restart-gateway'){
          startSampler();
          const deadline=Date.now()+75000;
          while(true){try{await api('/healthz');break;}catch(error){if(Date.now()>=deadline)throw error;await new Promise(r=>setTimeout(r,250));}}
          report.restartHttpReadyMs=Date.now()-e.started;
          report.afterRestartControls={};
          const scheduleKey=items=>JSON.stringify(items.map(s=>[s.start_ms,s.end_ms,s.repeat_seconds??null]));
          for(const alias of aliases){
            const actual=await api('/api/recording/'+alias),expected=report.recordingControls[alias];
            report.afterRestartControls[alias]=actual;
            if(actual.mode!==expected.mode||scheduleKey(actual.schedules)!==scheduleKey(expected.schedules))report.failures.push({kind:'recording-intent-changed',alias});
          }
        }
        console.log(JSON.stringify({event:e.kind,camera:e.camera,elapsed}));
      }
      if(e.done&&e.duration&&!e.restored&&Date.now()>=e.started+e.duration*1000){
        action(e.kind==='blackhole'?'restore':'source-on',e.camera);e.restored=Date.now();
        report.events.push({kind:'restore',camera:e.camera,at:e.restored,elapsed:(e.restored-start)/1000});
      }
    }
    const sample={at:Date.now(),elapsed,players:[],streams:null,clips:{},failures:[]};
    for(const {mode,page} of pages){
      const players=await page.evaluate(()=>soakSnapshot());
      for(const player of players){
        const key=mode+'/'+player.id,old=previous.get(key);player.mode=mode;
        player.expectedOutage=sample.at<grace.get(player.id);
        if(old){
          player.fps=(player.generation===old.generation?player.frames-old.frames:player.frames)/((sample.at-old.at)/1000);
          player.badSample=!player.expectedOutage&&(player.state!=='live'||player.fps<5||player.width!==640||player.rms<0.0001);
          if(player.badSample){
            if(!badSince.has(key))badSince.set(key,sample.at);
            if(sample.at-badSince.get(key)>=10000)sample.failures.push({key,kind:'persistent-playback',...player});
          }else badSince.delete(key);
        }
        previous.set(key,{...player,at:sample.at});sample.players.push(player);
      }
    }
    try{
      sample.streams=(await api('/api/streams')).streams;
      for(const alias of aliases){
        const listing=await api('/api/recordings?stream='+alias+'&limit=1');sample.clips[alias]=listing.files[0]||null;
        const clip=sample.clips[alias],previousClip=previousClips.get(alias);
        if(clip&&previousClip&&clip.file!==previousClip.file){
          if(clip.start_ms-previousClip.end_ms>2000){
            const interval=await api(`/api/recordings?stream=${alias}&from=${previousClip.start_ms}&to=${clip.end_ms}&limit=1000`);
            sample.failures.push(...unexplainedGaps(interval.files.filter(c=>c.start_ms>=previousClip.start_ms&&c.start_ms<=clip.start_ms),alias,events));
          }
        }
        if(clip)previousClips.set(alias,clip);
        if(sample.at>=grace.get(alias)&&elapsed>35){
          const state=sample.streams.find(s=>s.id===alias);
          if(state.recording!=='recording'||!state.online||state.audio.conversion_errors>0)sample.failures.push({key:alias,kind:'recording-state',state});
          if(!sample.clips[alias]||sample.at-sample.clips[alias].end_ms>45000)sample.failures.push({key:alias,kind:'recording-not-advancing'});
        }
      }
    }catch(e){sample.apiError=e.message;if(!aliases.every(a=>sample.at<grace.get(a)))sample.failures.push({kind:'api',message:e.message});}
    report.samples.push(sample);report.failures.push(...sample.failures.map(f=>({...f,elapsed})));
    await appendFile(path.join(output,'samples.jsonl'),JSON.stringify(sample)+'\n');
    if(sample.failures.length&&process.env.UVMS_SOAK_FAIL_FAST==='1'){await save();console.log(JSON.stringify({failedSample:sample.failures,elapsed}));break;}
    if(report.samples.length%6===0){await save();console.log(JSON.stringify({elapsed:Math.round(elapsed),live:sample.players.filter(p=>p.state==='live').length,failures:report.failures.length,rss:report.metrics.at(-1)?.rss_mib}));}
    await new Promise(r=>setTimeout(r,5000));
  }
  report.measuredSeconds=(Date.now()-start)/1000;
  if(stopRequested)report.failures.push({kind:'interrupted',message:'Test stopped before its requested duration'});
}catch(e){report.failures.push({kind:'fatal',message:e.stack});}
finally{
  for(const e of events.filter(e=>e.done&&e.duration&&!e.restored)){try{action(e.kind==='blackhole'?'restore':'source-on',e.camera);}catch{}}
  for(const {mode,page} of pages){report.mediaErrors.push({mode,errors:await page.evaluate(()=>soakErrors).catch(()=>[])});}
  await browser.close();
  for(const alias of aliases){try{await api('/api/recording/'+alias,{mode:'off',schedules:[]});}catch(e){report.failures.push({kind:'stop-recording',message:e.message});}}
  // Let HLS idle (15s) and shared input idle (30s) expire, recording the resource floor.
  await new Promise(r=>setTimeout(r,50000));
  report.finalStreams=await api('/api/streams').catch(()=>null);
  report.finalClips=await allClips().catch(()=>[]);
  const finalIndex=new Map(report.finalClips.map(c=>[c.stream+'/'+c.file,JSON.stringify(c)]));
  if(report.beforeRestartClips?.some(old=>finalIndex.get(old.stream+'/'+old.file)!==JSON.stringify(old)))report.failures.push({kind:'recording-index-changed-after-restart'});
  report.finalMetrics=report.metrics.at(-1);sampler.kill();
  if(report.finalMetrics?.source_inputs!==0)report.failures.push({kind:'input-not-released',metrics:report.finalMetrics});
  report.finishedAt=new Date().toISOString();report.passed=report.failures.length===0;await save();
}
console.log(JSON.stringify({passed:report.passed,seconds:report.measuredSeconds,failures:report.failures.length,output}));
if(!report.passed)process.exitCode=1;
