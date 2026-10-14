param([Parameter(Mandatory=$true)][string]$Path,[Parameter(Mandatory=$true)][string]$Output)
# Reads a standalone MSFT type library through OleAut32; never instantiates target COM classes.
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class StaticTypeLib {
 [DllImport("oleaut32.dll", CharSet=CharSet.Unicode, PreserveSig=false)]
 public static extern void LoadTypeLibEx(string path, int regKind, out ITypeLib lib);
 static string Describe(TYPEDESC t) {
  if(t.vt==26)return Describe(Marshal.PtrToStructure<TYPEDESC>(t.lpValue))+"*";
  return ((VarEnum)t.vt).ToString();
 }
 public static object Read(string path) {
  ITypeLib lib; LoadTypeLibEx(path,2,out lib);
  var items=new List<object>();
  for(int i=0;i<lib.GetTypeInfoCount();i++) {
   ITypeInfo info; lib.GetTypeInfo(i,out info);
   IntPtr ap; info.GetTypeAttr(out ap);
   var a=Marshal.PtrToStructure<TYPEATTR>(ap);
   string name,doc,help; int ctx; info.GetDocumentation(-1,out name,out doc,out ctx,out help);
   var funcs=new List<object>();
   for(int j=0;j<a.cFuncs;j++) {
    IntPtr fp;info.GetFuncDesc(j,out fp);var f=Marshal.PtrToStructure<FUNCDESC>(fp);
    var names=new string[32];int count;info.GetNames(f.memid,names,32,out count);Array.Resize(ref names,count);
    var parameters=new List<object>();
    for(int k=0;k<f.cParams;k++) {
     var e=Marshal.PtrToStructure<ELEMDESC>(IntPtr.Add(f.lprgelemdescParam,k*Marshal.SizeOf<ELEMDESC>()));
     parameters.Add(new {name=k+1<count?names[k+1]:null,type=Describe(e.tdesc),flags=e.desc.paramdesc.wParamFlags.ToString()});
    }
    funcs.Add(new {memberId=f.memid,vtableOffset=f.oVft,kind=f.funckind.ToString(),returnType=Describe(f.elemdescFunc.tdesc),names,parameters,parameterCount=f.cParams});
    info.ReleaseFuncDesc(fp);
   }
   items.Add(new {name,guid=a.guid.ToString(),kind=a.typekind.ToString(),functions=funcs,implementedTypes=a.cImplTypes});
   info.ReleaseTypeAttr(ap);
  }
  return items;
 }
}
'@
$items=[StaticTypeLib]::Read((Resolve-Path -LiteralPath $Path).Path)
@{source=(Resolve-Path -LiteralPath $Path).Path;sha256=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLower();registration='REGKIND_NONE';items=$items;limitations=@('Static type-library metadata only; no target COM object is instantiated and no original DLL is loaded.')} | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $Output
