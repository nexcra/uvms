import {chromium} from 'playwright';
import {mkdir,writeFile} from 'node:fs/promises';
const output=process.env.UVMS_LABEL_OUTPUT||'test-results/soak/labels';
await mkdir(output,{recursive:true});
const browser=await chromium.launch({executablePath:'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',headless:true});
const page=await browser.newPage({viewport:{width:1440,height:1100}});
try{
  await page.goto('http://127.0.0.1:8099');
  await page.waitForSelector('#camera-choices input[value="opus"]',{state:'attached'});
  await page.locator('button[data-layout="4"]').click();
  await page.locator('#connect').click();
  await page.waitForTimeout(6500);
  const result=await page.evaluate(async()=>{
    const streams=(await(await fetch('/api/streams')).json()).streams;
    const rows=[...document.querySelectorAll('.camera-tile')].map(tile=>({id:tile.dataset.camera,label:tile.querySelector('.camera-rec').textContent,state:streams.find(s=>s.id===tile.dataset.camera)?.recording}));
    return {rows,passed:rows.length===4&&rows.some(r=>r.state==='recording')&&rows.every(r=>r.state!=='recording'||r.label==='● 녹화 중')};
  });
  await page.screenshot({path:output+'/wall.png',fullPage:true});
  await writeFile(output+'/result.json',JSON.stringify(result,null,2));console.log(JSON.stringify(result));
  if(!result.passed)process.exitCode=1;
}finally{await browser.close();}
