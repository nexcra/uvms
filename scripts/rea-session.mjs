// Persistent REA MCP client for reproducible native analysis; target is never executed.
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { mkdir, readdir, readFile, writeFile, rename } from 'node:fs/promises';
import path from 'node:path';
const [packageRoot, sessionDir] = process.argv.slice(2);
if (!packageRoot || !sessionDir) throw new Error('Usage: node rea-session.mjs REA_PACKAGE_ROOT SESSION_DIR');
const require = createRequire(path.join(packageRoot, 'package.json'));
const { Client } = await import(pathToFileURL(require.resolve('@modelcontextprotocol/client')));
const { StdioClientTransport } = await import(pathToFileURL(require.resolve('@modelcontextprotocol/client/stdio')));
const client = new Client({name:'uvms-native-investigation',version:'1.0.0'});
const transport = new StdioClientTransport({command:process.execPath,args:[path.join(packageRoot,'scripts','rea.mjs'),'--mcp'],env:process.env,stderr:'inherit'});
await mkdir(sessionDir,{recursive:true});
await client.connect(transport);
await writeFile(path.join(sessionDir,'tools.json'),JSON.stringify(await client.listTools(),null,2));
console.log('REA session ready: '+sessionDir);
try {
  while (true) {
    const files=(await readdir(sessionDir)).filter(n=>n.endsWith('.request.json')).sort();
    for (const file of files) {
      const stem=file.slice(0,-13);
      const request=JSON.parse(await readFile(path.join(sessionDir,file),'utf8'));
      if(request.name==='EXIT') { await client.close(); process.exit(0); }
      console.log('START '+stem+' '+request.name);
      let result;
      try { result=await client.callTool(request,{timeout:900000}); }
      catch(e) { result={clientError:String(e)}; }
      await writeFile(path.join(sessionDir,stem+'.response.json'),JSON.stringify(result,null,2));
      for (const [i,block] of (result.content??[]).entries()) {
        if(block.type==='text') { try { await writeFile(path.join(sessionDir,stem+`.evidence${i||''}.json`),JSON.stringify(JSON.parse(block.text),null,2)); } catch {} }
      }
      await rename(path.join(sessionDir,file),path.join(sessionDir,stem+'.done.json'));
      console.log('DONE '+stem);
    }
    await new Promise(r=>setTimeout(r,300));
  }
} finally { await client.close(); }
