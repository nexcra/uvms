import { browserPath } from "./browser-path.mjs";
// Repeated pre-offer disconnects must return the Java gateway's session permits.
import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
const browser = await chromium.launch({executablePath: browserPath,headless:true});
const page = await browser.newPage();
const result = {passed:false};
const base = process.env.UVMS_TEST_URL || 'http://127.0.0.1:8088';
const output = process.env.UVMS_TEST_OUTPUT || 'test-results/java-sessions';
try {
  await page.goto(base+'/healthz');
  result.closedBeforeOffer = await page.evaluate(async () => {
    async function once() {
      return new Promise((resolve,reject) => {
        const ws = new WebSocket(`ws://${location.host}/webrtc_playnow/singleport/udp/camera`);
        let codec=false;
        const timer=setTimeout(()=>{ws.close();reject(Error('codec timeout'));},10000);
        ws.onmessage=event=>{if(event.data==='H264|-|-|'){codec=true;ws.close();}};
        ws.onerror=()=>{clearTimeout(timer);reject(Error('WebSocket refused'));};
        ws.onclose=()=>{clearTimeout(timer);codec?resolve(1):reject(Error('closed before codec'));};
      });
    }
    let count=0;
    for(let batch=0;batch<20;batch++) {
      count+=(await Promise.all(Array.from({length:4},once))).length;
      await new Promise(resolve=>setTimeout(resolve,25));
    }
    return count;
  });
  result.passed=result.closedBeforeOffer===80;
} catch(error) {result.error=error.message;}
finally {
  await browser.close();
  await mkdir(output,{recursive:true});
  await writeFile(output+'/result.json',JSON.stringify(result,null,2));
  console.log(JSON.stringify(result));
}
if(!result.passed)process.exitCode=1;
