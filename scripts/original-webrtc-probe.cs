// Controlled oracle, not production code. Calls only the selected original DLL.
using System;
using System.IO;
using System.Linq;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Diagnostics;
using System.Threading;
using System.Security.Cryptography;
public static class OriginalWebRtcProbe {
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr LoadLibraryExW(string p,IntPtr f,uint flags);
 [DllImport("kernel32.dll",CharSet=CharSet.Ansi,SetLastError=true)] static extern IntPtr GetProcAddress(IntPtr h,string name);
 [DllImport("ole32.dll")] static extern int CoInitializeEx(IntPtr p,uint flags);
 [DllImport("ole32.dll")] static extern void CoTaskMemFree(IntPtr p);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int ClassObject(ref Guid clsid,ref Guid iid,out IntPtr factory);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreateInstance(IntPtr self,IntPtr outer,ref Guid iid,out IntPtr obj);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int QueryInterface(IntPtr self,ref Guid iid,out IntPtr obj);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate uint Release(IntPtr self);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int MediaInit(IntPtr self,IntPtr types);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int Frame(IntPtr self,IntPtr descriptor);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int Terminate(IntPtr self);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int InitClient(IntPtr self,IntPtr sdp,IntPtr protocol,out IntPtr answer,out IntPtr candidate);
 [UnmanagedFunctionPointer(CallingConvention.StdCall,CharSet=CharSet.Unicode)] delegate int RtspInit(IntPtr self,[MarshalAs(UnmanagedType.LPWStr)] string options,IntPtr types,IntPtr info);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int RtspPull(IntPtr self,IntPtr unused,out IntPtr sample);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int RtspStop(IntPtr self,IntPtr nonNull);
 static T Slot<T>(IntPtr p,int n) where T:Delegate => Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(p),n*8));
 static void Check(int hr,string name){Console.WriteLine(name+" HRESULT=0x"+hr.ToString("x8"));if(hr<0)Marshal.ThrowExceptionForHR(hr);}
 static IntPtr Alloc(byte[] data){var p=Marshal.AllocHGlobal(data.Length);Marshal.Copy(data,0,p,data.Length);return p;}
 static void Save(string output,string name,object value){File.WriteAllText(Path.Combine(output,name),JsonSerializer.Serialize(value,new JsonSerializerOptions{WriteIndented=true}));}
 static byte[] Normalize(byte[] avcc,out int count){
  // Recovered UMediaServer FUN_140025dc0: preserve NAL bodies, replace four-byte
  // AVCC lengths with Annex-B starts; append count and LE offset/length pairs.
  var offsets=new List<int>();var lengths=new List<int>();int pos=0;
  while(pos<avcc.Length){
   if(pos+4>avcc.Length||offsets.Count>=60)throw new Exception("Fixture exceeds original NAL normalization boundary");
   uint n=((uint)avcc[pos]<<24)|((uint)avcc[pos+1]<<16)|((uint)avcc[pos+2]<<8)|avcc[pos+3];
   if(n==0||n>avcc.Length-pos-4)throw new Exception("Invalid AVCC capture");
   offsets.Add(pos+4);lengths.Add((int)n);pos+=4+(int)n;
  }
  count=offsets.Count;var b=new byte[avcc.Length+4+count*8];Array.Copy(avcc,b,avcc.Length);
  Array.Copy(BitConverter.GetBytes(count),0,b,avcc.Length,4);
  for(int i=0;i<count;i++){
   int prefix=offsets[i]-4;b[prefix]=b[prefix+1]=b[prefix+2]=0;b[prefix+3]=1;
   Array.Copy(BitConverter.GetBytes(offsets[i]),0,b,avcc.Length+4+i*8,4);
   Array.Copy(BitConverter.GetBytes(lengths[i]),0,b,avcc.Length+8+i*8,4);
   if(!b.AsSpan(offsets[i],lengths[i]).SequenceEqual(avcc.AsSpan(offsets[i],lengths[i])))throw new Exception("NAL mutation");
  }
  return b;
 }
 public static void Run(string dll,string capture,string output,bool live){
  if(IntPtr.Size!=8)throw new Exception("x64 required");Check(CoInitializeEx(IntPtr.Zero,0),"CoInitializeEx");
  var module=LoadLibraryExW(dll,IntPtr.Zero,0x100|0x1000);if(module==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();
  var get=Marshal.GetDelegateForFunctionPointer<ClassObject>(GetProcAddress(module,"DllGetClassObject"));
  var clsid=new Guid("8c5a8556-d17d-4303-b66a-b5f94f3ed9cc");var cf=new Guid("00000001-0000-0000-c000-000000000046");var playing=new Guid("e403a754-6df0-4c75-9e86-d33bf5c25008");var media=new Guid("8a607e33-1e5b-47de-a340-82132d8a2f15");
  IntPtr factory,client,sink;Check(get(ref clsid,ref cf,out factory),"DllGetClassObject");Check(Slot<CreateInstance>(factory,3)(factory,IntPtr.Zero,ref playing,out client),"CreateInstance");Check(Slot<QueryInterface>(client,0)(client,ref media,out sink),"QueryInterface IMediaFlip");
  var allocations=new List<IntPtr>();var typeBytes=new byte[176];IntPtr provider=IntPtr.Zero;
  var types=Alloc(typeBytes);allocations.Add(types);
  if(live){
   string rtspDll=Path.Combine(Path.GetDirectoryName(dll),"URTSPClient.dll");string rtspHash=Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(rtspDll)));
   if(rtspHash!="056726F883B96982BC303AE84CE9B3CB8D4C7C3640E82D42EF96BB08976AD148")throw new Exception("Original RTSP DLL identity mismatch");
   var rtspModule=LoadLibraryExW(rtspDll,IntPtr.Zero,0x100|0x1000);if(rtspModule==IntPtr.Zero)throw new System.ComponentModel.Win32Exception();
   var rtspGet=Marshal.GetDelegateForFunctionPointer<ClassObject>(GetProcAddress(rtspModule,"DllGetClassObject"));var rtspClsid=new Guid("022f2113-b1bd-4326-9cc7-ddf80abd245c");var providerIid=new Guid("7c5e98af-e07e-4a9c-b508-c29039cf8ba1");IntPtr rtspFactory;
   Check(rtspGet(ref rtspClsid,ref cf,out rtspFactory),"RTSP DllGetClassObject");Check(Slot<CreateInstance>(rtspFactory,3)(rtspFactory,IntPtr.Zero,ref providerIid,out provider),"RTSP CreateInstance");
   var info=Alloc(new byte[16]);allocations.Add(info);string options="URL=rtsp://127.0.0.1:8554/camera|Transport=TCP|VideoStream=1|AudioStream=0|RTCPSynch=0";
   Check(Slot<RtspInit>(provider,3)(provider,options,types,info),"RTSP Init");
   Save(output,"live-source.json",new{url="rtsp://127.0.0.1:8554/camera",options,rtspDll,rtspHash});
  }else for(int i=0;i<2;i++){
   var t=File.ReadAllBytes(Path.Combine(capture,"media-type-"+i+".bin"));if(t.Length!=88)throw new Exception("AM_MEDIA_TYPE size");
   Array.Clear(t,64,8);Array.Clear(t,80,8);var formatPath=Path.Combine(capture,"media-format-"+i+".bin");
   if(File.Exists(formatPath)){var f=File.ReadAllBytes(formatPath);var ptr=Alloc(f);allocations.Add(ptr);Array.Copy(BitConverter.GetBytes(ptr.ToInt64()),0,t,80,8);}
   Array.Copy(t,0,typeBytes,i*88,88);
  }
  if(!live)Marshal.Copy(typeBytes,0,types,typeBytes.Length);
  Check(Slot<MediaInit>(sink,3)(sink,types),"IMediaFlip.Init");
  File.WriteAllText(Path.Combine(output,"ready.json"),"{\"ready\":true}");
  var timeout=Stopwatch.StartNew();string offerPath=Path.Combine(output,"offer.json");
  while(!File.Exists(offerPath)){if(timeout.ElapsedMilliseconds>12000)throw new Exception("Offer deadline");Thread.Sleep(20);}
  var offer=Marshal.StringToBSTR(File.ReadAllText(offerPath));var protocol=Marshal.StringToBSTR("udp");IntPtr answer,candidate;
  int initHr=Slot<InitClient>(client,3)(client,offer,protocol,out answer,out candidate);
  var state=new byte[0x180];Marshal.Copy(client,state,0,state.Length);File.WriteAllBytes(Path.Combine(output,"client-state-after-init.bin"),state);
  Save(output,"init-state.json",new{hresult=initHr,peerPointer=Marshal.ReadIntPtr(client,0x58).ToInt64(),transportPointer=Marshal.ReadIntPtr(client,0x60).ToInt64(),poolPointer=Marshal.ReadIntPtr(client,0x68).ToInt64(),flags=new[]{state[0xe0],state[0xe1],state[0xe2],state[0xe3]},videoCodecLength=Marshal.ReadInt64(client,0x118),audioCodecLength=Marshal.ReadInt64(client,0x138)});
  Check(initHr,"IPlayingPeerClient.InitClient");
  var answerText=Marshal.PtrToStringBSTR(answer);var candidateText=Marshal.PtrToStringBSTR(candidate);
  Save(output,"answer.json",new{answer=JsonDocument.Parse(answerText).RootElement.Clone(),candidate=JsonDocument.Parse(candidateText).RootElement.Clone()});
  Marshal.FreeBSTR(offer);Marshal.FreeBSTR(protocol);Marshal.FreeBSTR(answer);Marshal.FreeBSTR(candidate);
  JsonElement[] selected=Array.Empty<JsonElement>();
  if(!live){var frames=File.ReadAllLines(Path.Combine(capture,"frames.jsonl")).Select(s=>JsonDocument.Parse(s).RootElement.Clone()).ToArray();int first=Array.FindIndex(frames,f=>f.GetProperty("keyframe").GetInt32()!=0);if(first<0)throw new Exception("No keyframe fixture");selected=frames.Skip(first).ToArray();}
  var descriptor=Alloc(new byte[48]);allocations.Add(descriptor);
  var submit=Slot<Frame>(sink,5);var watch=Stopwatch.StartNew();int sent=0,notReady=0,index=0;long ordinal=0;long firstTimestamp=-1;
  using(var log=new StreamWriter(Path.Combine(output,"submitted.jsonl"),false)){
   while(watch.ElapsedMilliseconds<20000&&!File.Exists(Path.Combine(output,"stop"))){
    byte[] avcc;long timestamp;byte keyframe;int sourceIndex;
    if(live){
     IntPtr sample;int pullHr=Slot<RtspPull>(provider,5)(provider,IntPtr.Zero,out sample);if(pullHr<0)Check(pullHr,"RTSP GetSample");if(pullHr!=0||sample==IntPtr.Zero){Thread.Sleep(5);continue;}
     IntPtr data=Marshal.ReadIntPtr(sample,40);
     try{int kind=Marshal.ReadInt32(sample),length=Marshal.ReadInt32(sample,32);if(kind!=2||length<0||length>16777216||data==IntPtr.Zero)throw new Exception("Unexpected live sample");avcc=new byte[length];Marshal.Copy(data,avcc,0,length);timestamp=Marshal.ReadInt64(sample,8);keyframe=Marshal.ReadByte(sample,24);sourceIndex=(int)ordinal;}finally{if(data!=IntPtr.Zero)CoTaskMemFree(data);CoTaskMemFree(sample);}
    }else{var f=selected[index];avcc=File.ReadAllBytes(Path.Combine(capture,f.GetProperty("file").GetString()));timestamp=ordinal*10000000/15;keyframe=(byte)f.GetProperty("keyframe").GetInt32();sourceIndex=f.GetProperty("index").GetInt32();}
    if(firstTimestamp<0)firstTimestamp=timestamp;
    int nals;var normalized=Normalize(avcc,out nals);var payload=Alloc(normalized);
    try{
     Marshal.WriteInt32(descriptor,0,2);Marshal.WriteInt64(descriptor,8,timestamp);Marshal.WriteInt64(descriptor,16,timestamp+10000000/15);Marshal.WriteByte(descriptor,24,keyframe);Marshal.WriteInt32(descriptor,32,avcc.Length);Marshal.WriteIntPtr(descriptor,40,payload);
     int hr=submit(sink,descriptor);if(hr<0)Check(hr,"IMediaFlip frame");if(hr==1)notReady++;else sent++;
     log.WriteLine(JsonSerializer.Serialize(new{ordinal,sourceIndex,timestamp100ns=timestamp,keyframe,hresult=hr,nalCount=nals,payloadLength=avcc.Length,normalizedLength=normalized.Length}));log.Flush();
    }finally{Marshal.FreeHGlobal(payload);}
    ordinal++;if(!live)index=(index+1)%selected.Length;long due=live?(timestamp-firstTimestamp)/10000:ordinal*1000/15;while(watch.ElapsedMilliseconds<due)Thread.Sleep(2);
   }
  }
  Check(Slot<Terminate>(client,4)(client),"IPlayingPeerClient.Terminate");
  if(live)Check(Slot<RtspStop>(provider,4)(provider,types),"RTSP Stop");
  Save(output,"result.json",new{success=sent>0,sent,notReady,elapsedMs=watch.ElapsedMilliseconds,normalizer="UMediaServer 0x140025dc0 AVCC branch; NAL payload verified unchanged",captureReplay=!live,liveRtsp=live});
  // Keep DLL loaded through process exit so its original background threads cannot
  // race FreeLibrary. Terminate ends peer; this entire worker is time-bounded.
  foreach(var p in allocations)Marshal.FreeHGlobal(p);
  Console.WriteLine("Original WebRTC frames accepted="+sent+", notReady="+notReady);
 }
}
