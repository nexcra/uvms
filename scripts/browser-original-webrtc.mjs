import {chromium} from 'playwright';
import {mkdir,readFile,writeFile,rename} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import path from 'node:path';
const output=path.resolve(process.argv[2]||'evidence/runtime/webrtc-original');await mkdir(output,{recursive:true});
const browser=await chromium.launch({executablePath:'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',headless:true,args:['--disable-features=WebRtcHideLocalIpsWithMdns']});
const live=process.argv.includes('--live');
const page=await browser.newPage();const result={passed:false,authority:live?'real browser against original URTSPClient.dll + recovered server normalizer + original UnrealWebRTC.dll':'real browser against original UnrealWebRTC.dll; original RTSP capture replay',client:'minimal browser oracle matching original SDK H264 and SDP filters',errors:[]};
page.on('pageerror',e=>result.errors.push(e.message));
try{
 await page.goto('http://127.0.0.1:8080/');
 await page.evaluate(()=>{document.body.innerHTML='<video id="original" autoplay muted playsinline style="width:960px"></video>';});
 const offer=await page.evaluate(async()=>{
  const pc=new RTCPeerConnection({iceServers:[]});window.originalPc=pc;window.originalEvents=[];
  for(const kind of ['connectionstatechange','iceconnectionstatechange','signalingstatechange'])pc.addEventListener(kind,()=>window.originalEvents.push({kind,connection:pc.connectionState,ice:pc.iceConnectionState,signal:pc.signalingState}));
  pc.ontrack=e=>{document.getElementById('original').srcObject=new MediaStream([e.track]);};
  const t=pc.addTransceiver('video',{direction:'recvonly'});const caps=RTCRtpReceiver.getCapabilities('video');
  // Same codec boundary as original SDK setCodec/EnsureSupportedProfile: one H264
  // profile starting42, and remove extmap-allow-mixed before SetLocalDescription.
  const h264=caps.codecs.find(c=>c.mimeType.toLowerCase()==='video/h264'&&c.sdpFmtpLine?.includes('profile-level-id=42')&&c.sdpFmtpLine?.includes('packetization-mode=1'));
  if(!h264)throw new Error('No original SDK compatible H264 capability');t.setCodecPreferences([h264]);
  const description=await pc.createOffer();description.sdp=description.sdp.replaceAll('a=extmap-allow-mixed\r\n','');await pc.setLocalDescription(description);
  if(pc.iceGatheringState!=='complete')await new Promise((resolve,reject)=>{const timeout=setTimeout(()=>reject(new Error('ICE timeout')),8000);pc.onicegatheringstatechange=()=>{if(pc.iceGatheringState==='complete'){clearTimeout(timeout);resolve();}};});
  return pc.localDescription.toJSON();
 });
 await writeFile(path.join(output,'offer.tmp'),JSON.stringify(offer));await rename(path.join(output,'offer.tmp'),path.join(output,'offer.json'));
 const deadline=Date.now()+15000;while(!existsSync(path.join(output,'answer.json'))){if(Date.now()>deadline)throw new Error('Original DLL answer deadline');await new Promise(r=>setTimeout(r,100));}
 const answer=JSON.parse(await readFile(path.join(output,'answer.json'),'utf8'));
 await page.evaluate(async r=>{await window.originalPc.setRemoteDescription(r.answer);await window.originalPc.addIceCandidate(r.candidate);},answer);
 await page.waitForFunction(()=>document.getElementById('original').videoWidth>0&&document.getElementById('original').getVideoPlaybackQuality().totalVideoFrames>30,null,{timeout:16000});
 result.video=await page.evaluate(()=>{const v=document.getElementById('original');return{width:v.videoWidth,height:v.videoHeight,time:v.currentTime,frames:v.getVideoPlaybackQuality().totalVideoFrames};});
 result.passed=true;await page.screenshot({path:path.join(output,'original-player.png')});
}catch(e){result.error=e.message;}
finally{
 result.events=await page.evaluate(()=>window.originalEvents||[]).catch(()=>[]);
 result.stats=await page.evaluate(async()=>{if(!window.originalPc)return[];return [...(await window.originalPc.getStats()).values()].filter(s=>['inbound-rtp','candidate-pair','transport','codec'].includes(s.type));}).catch(()=>[]);
 await page.evaluate(()=>window.originalPc?.close()).catch(()=>{});
 await writeFile(path.join(output,'browser.json'),JSON.stringify(result,null,2));await writeFile(path.join(output,'stop'),'browser completed');
 await browser.close();console.log(JSON.stringify(result,null,2));
}
if(!result.passed)process.exitCode=1;
