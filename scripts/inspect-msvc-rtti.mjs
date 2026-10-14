// Read-only PE32+ MSVC RTTI / vtable inventory. Does not execute DLL code.
import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
const [file,pattern,out]=process.argv.slice(2), b=readFileSync(file),pe=b.readUInt32LE(0x3c),opt=pe+24;
const base=b.readBigUInt64LE(opt+24),n=b.readUInt16LE(pe+6),st=opt+b.readUInt16LE(pe+20),sections=[];
for(let i=0;i<n;i++){const p=st+i*40;sections.push({name:b.toString('ascii',p,p+8).replaceAll('\0',''),rva:b.readUInt32LE(p+12),raw:b.readUInt32LE(p+20),size:b.readUInt32LE(p+16)});}
const hx=x=>'0x'+BigInt(x).toString(16), rva=p=>{const s=sections.find(s=>p>=s.raw&&p<s.raw+s.size);return s?s.rva+p-s.raw:null;};
const off=r=>{const s=sections.find(s=>r>=s.rva&&r<s.rva+s.size);return s?s.raw+r-s.rva:null;};
const rx=new RegExp(pattern),items=[];
for(const m of b.toString('latin1').matchAll(/\.\?AV[^\x00]+\x00/g)) {
 const name=m[0].slice(0,-1);if(!rx.test(name))continue;
 const td=rva(m.index-16), cols=[];
 for(const s of sections.filter(s=>s.name==='.rdata'))for(let p=s.raw;p+24<s.raw+s.size;p+=4){
  if(b.readUInt32LE(p)!==1||b.readUInt32LE(p+12)!==td||b.readUInt32LE(p+20)!==rva(p))continue;
  const colRva=rva(p),vtables=[];
  for(let q=s.raw;q+8<s.raw+s.size;q+=8)if(b.readBigUInt64LE(q)===base+BigInt(colRva)){
   const entries=[];for(let k=q+8;k<q+8+8*40;k+=8){const va=b.readBigUInt64LE(k),r=Number(va-base),of=off(r),seg=sections.find(s=>r>=s.rva&&r<s.rva+s.size);if(of===null||seg?.name!=='.text')break;entries.push({slot:(k-q-8)/8,va:hx(va),fileOffset:of});}
   vtables.push({address:hx(base+BigInt(rva(q+8))),entries});
  }
  cols.push({address:hx(base+BigInt(colRva)),objectOffset:b.readUInt32LE(p+4),vtables});
 }
 items.push({name,nameFileOffset:m.index,typeDescriptor:hx(base+BigInt(td)),completeObjectLocators:cols});
}
const result={source:file,sha256:createHash('sha256').update(b).digest('hex'),imageBase:hx(base),items,limitations:['Offline MSVC RTTI structure matching; derived vtable candidates, no execution or recovered function semantics.']};
writeFileSync(out,JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
