// Offline PE imports/exports and bounded string observations; never loads target code.
import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
const [input,output] = process.argv.slice(2);
if (!input || !output) throw new Error('Usage: node inspect-pe.mjs INPUT OUTPUT');
const b=readFileSync(input), pe=b.readUInt32LE(0x3c);
if(b.toString('ascii',pe,pe+4)!=='PE\0\0') throw new Error('Not PE');
const count=b.readUInt16LE(pe+6), opt=pe+24, size=b.readUInt16LE(pe+20), is64=b.readUInt16LE(opt)===0x20b;
const dir=opt+(is64?112:96), sections=[];
for(let i=0;i<count;i++) { const p=opt+size+i*40; sections.push({name:b.toString('ascii',p,p+8).replace(/\0/g,''),size:b.readUInt32LE(p+16),va:b.readUInt32LE(p+12),raw:b.readUInt32LE(p+20)}); }
function offset(rva) {const s=sections.find(s=>rva>=s.va&&rva-s.va<s.size); if(!s)throw new Error(`Unmapped RVA ${rva}`);return s.raw+rva-s.va;}
function str(p){let end=b.indexOf(0,p);if(end<0)end=b.length;return b.toString('ascii',p,end);}
const imports=[],exports=[];
const ir=b.readUInt32LE(dir+8);
if(ir)for(let p=offset(ir);b.readUInt32LE(p+12);p+=20) {
 const name=str(offset(b.readUInt32LE(p+12))), thunk=offset(b.readUInt32LE(p)||b.readUInt32LE(p+16)), symbols=[];
 for(let q=thunk;;q+=is64?8:4) {const v=is64?b.readBigUInt64LE(q):BigInt(b.readUInt32LE(q));if(!v)break; const flag=is64?1n<<63n:1n<<31n; symbols.push(v&flag?`ordinal:${v&65535n}`:str(offset(Number(v))+2));}
 imports.push({name,symbols});
}
const er=b.readUInt32LE(dir);
if(er) {const e=offset(er), n=b.readUInt32LE(e+24), names=offset(b.readUInt32LE(e+32));for(let i=0;i<n;i++)exports.push(str(offset(b.readUInt32LE(names+i*4))));}
const strings=[];
const pattern=/webrtc|websocket|rtsp|playnow|H264|VP8|Opus|stun|turn:|DTLS|SRTP|libjingle/i;
const ascii=b.toString('latin1');
for(const m of ascii.matchAll(/[\x20-\x7e]{8,400}/g))if(pattern.test(m[0]))strings.push({offset:m.index,encoding:'ascii',value:m[0]});
writeFileSync(output,JSON.stringify({source:input,sha256:createHash('sha256').update(b).digest('hex'),machine:b.readUInt16LE(pe+4),sections,imports,exports,strings,limitations:['Static imports/exports and matching ASCII strings only; string presence does not establish reachable behavior.','No decompilation or target execution.']},null,2));
console.log(JSON.stringify({input,imports:imports.map(i=>i.name),exports,matchedStrings:strings.length}));
