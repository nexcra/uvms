param([switch]$Worker, [string]$OutputDirectory = 'D:\workspaces\uvms\evidence\runtime\rtsp-original')
$ErrorActionPreference = 'Stop'
$dllPath = 'D:\workspaces\uvms\evidence\extracted\URTSPClient.dll'
$expectedHash = '056726F883B96982BC303AE84CE9B3CB8D4C7C3640E82D42EF96BB08976AD148'
if ((Get-FileHash -LiteralPath $dllPath -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Target identity mismatch' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
if (-not $Worker) {
    $probeProcess = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @('-NoProfile','-MTA','-File',('"' + $PSCommandPath + '"'),'-Worker','-OutputDirectory',('"' + $OutputDirectory + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.txt') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.txt')
    $completed = $probeProcess.WaitForExit(15000)
    if (-not $completed) { Stop-Process -Id $probeProcess.Id -Force; $probeProcess.WaitForExit() }
    @{ target=$dllPath; sha256=$expectedHash.ToLower(); input='rtsp://127.0.0.1:8554/camera'; boundedMilliseconds=15000; timedOut=(-not $completed); exitCode=$probeProcess.ExitCode; authority='controlled native DLL execution; not REA evidence'; registryRegistration=$false } | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'run.json')
    Get-Content (Join-Path $OutputDirectory 'stdout.txt')
    Get-Content (Join-Path $OutputDirectory 'stderr.txt')
    exit
}
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Diagnostics;
using System.Collections.Generic;
using System.Text.Json;
public static class OriginalRtspProbe {
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
 [DllImport("kernel32.dll", CharSet=CharSet.Ansi, SetLastError=true)] static extern IntPtr GetProcAddress(IntPtr module, string name);
 [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
 [DllImport("ole32.dll")] static extern int CoInitializeEx(IntPtr reserved, uint flags);
 [DllImport("ole32.dll")] static extern void CoUninitialize();
 [DllImport("ole32.dll")] static extern void CoTaskMemFree(IntPtr memory);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int ClassObject(ref Guid clsid, ref Guid iid, out IntPtr factory);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreateInstance(IntPtr self, IntPtr outer, ref Guid iid, out IntPtr instance);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate uint Release(IntPtr self);
 [UnmanagedFunctionPointer(CallingConvention.StdCall, CharSet=CharSet.Unicode)] delegate int Init(IntPtr self, [MarshalAs(UnmanagedType.LPWStr)] string url, IntPtr mediaTypes, IntPtr info);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int Stop(IntPtr self, IntPtr unusedNonNull);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetSample(IntPtr self, IntPtr unused, out IntPtr sample);
 static T Slot<T>(IntPtr self,int index) where T:Delegate { return Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(self), index*8)); }
 static void Check(int hr,string name) { Console.WriteLine(name+" HRESULT=0x"+hr.ToString("x8")); if(hr<0) Marshal.ThrowExceptionForHR(hr); }
 static byte[] Bytes(IntPtr ptr,int length) { var b=new byte[length]; Marshal.Copy(ptr,b,0,length);return b; }
 static object MediaType(IntPtr mt,string output,int index) {
   int n=Marshal.ReadInt32(mt,72);IntPtr data=Marshal.ReadIntPtr(mt,80);
   if(n<0||n>1048576||(n>0&&data==IntPtr.Zero))throw new Exception("Invalid media format bounds");
   var bytes=Bytes(mt,88);File.WriteAllBytes(Path.Combine(output,"media-type-"+index+".bin"),bytes);
   if(n>0)File.WriteAllBytes(Path.Combine(output,"media-format-"+index+".bin"),Bytes(data,n));
   return new { index, major=new Guid(Bytes(mt,16)),subtype=new Guid(Bytes(IntPtr.Add(mt,16),16)),formatType=new Guid(Bytes(IntPtr.Add(mt,44),16)), formatLength=n };
 }
 public static void Run(string dll,string output) {
   if(IntPtr.Size!=8)throw new Exception("x64 required");
   int co=CoInitializeEx(IntPtr.Zero,0);Check(co,"CoInitializeEx");
   IntPtr module=IntPtr.Zero,factory=IntPtr.Zero,instance=IntPtr.Zero,types=IntPtr.Zero,info=IntPtr.Zero;
   bool initialized=false;
   try {
     module=LoadLibraryExW(dll,IntPtr.Zero,0x100|0x1000);if(module==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();
     var getClass=Marshal.GetDelegateForFunctionPointer<ClassObject>(GetProcAddress(module,"DllGetClassObject"));
     Guid clsid=new Guid("022f2113-b1bd-4326-9cc7-ddf80abd245c"), cf=new Guid("00000001-0000-0000-c000-000000000046"), provider=new Guid("7c5e98af-e07e-4a9c-b508-c29039cf8ba1");
     Check(getClass(ref clsid,ref cf,out factory),"DllGetClassObject");
     Check(Slot<CreateInstance>(factory,3)(factory,IntPtr.Zero,ref provider,out instance),"CreateInstance");
     types=Marshal.AllocHGlobal(176);Marshal.Copy(new byte[176],0,types,176);info=Marshal.AllocHGlobal(16);Marshal.Copy(new byte[16],0,info,16);
     string options="URL=rtsp://127.0.0.1:8554/camera|Transport=TCP|VideoStream=1|AudioStream=0|RTCPSynch=0";
     Check(Slot<Init>(instance,3)(instance,options,types,info),"Init");initialized=true;
     File.WriteAllText(Path.Combine(output,"metadata.json"),JsonSerializer.Serialize(new { options,types=new object[]{MediaType(types,output,0),MediaType(IntPtr.Add(types,88),output,1)},info=Convert.ToHexString(Bytes(info,16)) },new JsonSerializerOptions{WriteIndented=true}));
     var pull=Slot<GetSample>(instance,5);var watch=Stopwatch.StartNew();int count=0;int empty=0;
     using(var log=new StreamWriter(Path.Combine(output,"frames.jsonl"),false)) {
       while(watch.ElapsedMilliseconds<8500&&count<40) {
         IntPtr sample;int hr=pull(instance,IntPtr.Zero,out sample);if(hr<0)Check(hr,"GetSample");
         if(hr!=0||sample==IntPtr.Zero){empty++;System.Threading.Thread.Sleep(5);continue;}
         IntPtr data=IntPtr.Zero;
         try {
           int kind=Marshal.ReadInt32(sample),length=Marshal.ReadInt32(sample,32);data=Marshal.ReadIntPtr(sample,40);
           if((kind!=1&&kind!=2)||length<0||length>16777216||(length>0&&data==IntPtr.Zero))throw new Exception("Invalid sample bounds");
           byte[] payload=Bytes(data,length);string file="frame-"+count.ToString("D3")+".bin";File.WriteAllBytes(Path.Combine(output,file),payload);
           var nals=new List<object>();int cursor=0;
           while(cursor+4<=length&&nals.Count<128){uint n=((uint)payload[cursor]<<24)|((uint)payload[cursor+1]<<16)|((uint)payload[cursor+2]<<8)|payload[cursor+3];if(n==0||n>(uint)(length-cursor-4))break;nals.Add(new {offset=cursor,length=n,type=payload[cursor+4]&31});cursor+=4+(int)n;}
           log.WriteLine(JsonSerializer.Serialize(new {index=count,kind,start100ns=Marshal.ReadInt64(sample,8),end100ns=Marshal.ReadInt64(sample,16),keyframe=Marshal.ReadByte(sample,24),length,file,nals,parsedBytes=cursor,trailingBytes=length-cursor}));log.Flush();count++;
         } finally {if(data!=IntPtr.Zero)CoTaskMemFree(data);CoTaskMemFree(sample);}
       }
     }
     Console.WriteLine("Captured "+count+" original-DLL samples; empty polls="+empty);
     File.WriteAllText(Path.Combine(output,"result.json"),JsonSerializer.Serialize(new{sampleCount=count,emptyPolls=empty,elapsedMs=watch.ElapsedMilliseconds,success=count>0},new JsonSerializerOptions{WriteIndented=true}));
   } finally {
     if(initialized){Console.WriteLine("Stopping original client");Check(Slot<Stop>(instance,4)(instance,types),"Stop");}
     if(types!=IntPtr.Zero){for(int i=0;i<2;i++){var mt=IntPtr.Add(types,88*i);var format=Marshal.ReadIntPtr(mt,80);if(format!=IntPtr.Zero)CoTaskMemFree(format);}Marshal.FreeHGlobal(types);}
     if(info!=IntPtr.Zero)Marshal.FreeHGlobal(info);
     if(instance!=IntPtr.Zero)Slot<Release>(instance,2)(instance);
     if(factory!=IntPtr.Zero)Slot<Release>(factory,2)(factory);
     if(module!=IntPtr.Zero)FreeLibrary(module);
     if(co>=0)CoUninitialize();
   }
 }
}
'@
[OriginalRtspProbe]::Run($dllPath,$OutputDirectory)
