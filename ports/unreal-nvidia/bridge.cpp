// Direct NVIDIA Driver / NVDEC / NVENC APIs. No FFmpeg or software codec fallback.
#include <cuda.h>
#include "nvcuvid.h"
#include "nvEncodeAPI.h"
#include <dlfcn.h>
#include <algorithm>
#include <cstring>
#include <deque>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
void require(bool ok, const char* message) { if (!ok) throw std::runtime_error(message); }
void cu(CUresult status, const char* operation) {
    if (status != CUDA_SUCCESS) throw std::runtime_error(std::string(operation)+" failed (CUDA "+std::to_string(status)+")");
}
void nv(NVENCSTATUS status, const char* operation) {
    if (status != NV_ENC_SUCCESS) throw std::runtime_error(std::string(operation)+" failed (NVENC "+std::to_string(status)+")");
}
void error_text(char* target, size_t size, const char* text) {
    if (target && size) { std::strncpy(target,text,size-1); target[size-1]=0; }
}
struct Library {
    void* handle;
    explicit Library(const char* name): handle(dlopen(name,RTLD_NOW|RTLD_LOCAL)) {
        if (!handle) throw std::runtime_error(std::string("NVIDIA library unavailable: ")+name);
    }
    ~Library() { if (handle) dlclose(handle); }
    template<class T> T get(const char* name) {
        auto value = reinterpret_cast<T>(dlsym(handle,name));
        if (!value) throw std::runtime_error(std::string("NVIDIA API unavailable: ")+name);
        return value;
    }
};
struct Packet { std::vector<unsigned char> bytes; uint64_t timestamp; };
struct Engine {
    Library cuda{"libcuda.so.1"}, decode{"libnvcuvid.so.1"}, encode{"libnvidia-encode.so.1"};
    CUresult (*init)(unsigned)=cuda.get<decltype(init)>("cuInit");
    CUresult (*device_get)(CUdevice*,int)=cuda.get<decltype(device_get)>("cuDeviceGet");
    CUresult (*context_create)(CUcontext*,unsigned,CUdevice)=cuda.get<decltype(context_create)>("cuCtxCreate_v2");
    CUresult (*context_destroy)(CUcontext)=cuda.get<decltype(context_destroy)>("cuCtxDestroy_v2");
    CUresult (*context_set)(CUcontext)=cuda.get<decltype(context_set)>("cuCtxSetCurrent");
    decltype(&cuvidCreateVideoParser) parser_create=decode.get<decltype(parser_create)>("cuvidCreateVideoParser");
    decltype(&cuvidDestroyVideoParser) parser_destroy=decode.get<decltype(parser_destroy)>("cuvidDestroyVideoParser");
    decltype(&cuvidParseVideoData) parse=decode.get<decltype(parse)>("cuvidParseVideoData");
    decltype(&cuvidGetDecoderCaps) caps=decode.get<decltype(caps)>("cuvidGetDecoderCaps");
    decltype(&cuvidCreateDecoder) decoder_create=decode.get<decltype(decoder_create)>("cuvidCreateDecoder");
    decltype(&cuvidDestroyDecoder) decoder_destroy=decode.get<decltype(decoder_destroy)>("cuvidDestroyDecoder");
    decltype(&cuvidDecodePicture) decode_picture=decode.get<decltype(decode_picture)>("cuvidDecodePicture");
    decltype(&cuvidMapVideoFrame64) map_frame=decode.get<decltype(map_frame)>("cuvidMapVideoFrame64");
    decltype(&cuvidUnmapVideoFrame64) unmap_frame=decode.get<decltype(unmap_frame)>("cuvidUnmapVideoFrame64");
    NV_ENCODE_API_FUNCTION_LIST api{};
    CUcontext context=nullptr;
    CUvideoparser parser=nullptr;
    CUvideodecoder decoder=nullptr;
    void* encoder=nullptr;
    NV_ENC_OUTPUT_PTR bitstream=nullptr;
    unsigned width=0,height=0,coded_width=0,coded_height=0,surfaces=0,bitrate=0;
    unsigned fps_num=30,fps_den=1,rtp_duration=3000;
    bool initialized=false;
    std::string callback_error;
    std::deque<Packet> output;
    Engine() = default;
    ~Engine() {
        if (context) context_set(context);
        if (parser) parser_destroy(parser);
        if (encoder && bitstream) api.nvEncDestroyBitstreamBuffer(encoder,bitstream);
        if (encoder) api.nvEncDestroyEncoder(encoder);
        if (decoder) decoder_destroy(decoder);
        if (context) context_destroy(context);
    }
    void open(unsigned ordinal,unsigned rate) {
        bitrate=rate;
        cu(init(0),"cuInit"); CUdevice device;
        cu(device_get(&device,static_cast<int>(ordinal)),"cuDeviceGet");
        cu(context_create(&context,CU_CTX_SCHED_BLOCKING_SYNC,device),"cuCtxCreate");
        CUVIDDECODECAPS capability{};
        capability.eCodecType=cudaVideoCodec_HEVC; capability.eChromaFormat=cudaVideoChromaFormat_420;
        cu(caps(&capability),"cuvidGetDecoderCaps");
        require(capability.bIsSupported && (capability.nOutputFormatMask & (1<<cudaVideoSurfaceFormat_NV12)),"GPU lacks HEVC 8-bit NV12 decoding");
        auto max_version=encode.get<NVENCSTATUS(NVENCAPI*)(uint32_t*)>("NvEncodeAPIGetMaxSupportedVersion");
        uint32_t version=0; nv(max_version(&version),"NvEncodeAPIGetMaxSupportedVersion");
        require(version >= ((NVENCAPI_MAJOR_VERSION<<4)|NVENCAPI_MINOR_VERSION),"NVIDIA driver is older than NVENC API 12.0");
        api.version=NV_ENCODE_API_FUNCTION_LIST_VER;
        auto create=encode.get<NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*)>("NvEncodeAPICreateInstance");
        nv(create(&api),"NvEncodeAPICreateInstance");
        NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS session{}; session.version=NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
        session.device=context; session.deviceType=NV_ENC_DEVICE_TYPE_CUDA; session.apiVersion=NVENCAPI_VERSION;
        nv(api.nvEncOpenEncodeSessionEx(&session,&encoder),"nvEncOpenEncodeSessionEx");
        uint32_t count=0; nv(api.nvEncGetEncodeGUIDCount(encoder,&count),"nvEncGetEncodeGUIDCount");
        require(count>0 && count<256,"Invalid NVENC codec count"); std::vector<GUID> codecs(count);
        nv(api.nvEncGetEncodeGUIDs(encoder,codecs.data(),count,&count),"nvEncGetEncodeGUIDs");
        require(std::any_of(codecs.begin(),codecs.end(),[](const GUID& g){return std::memcmp(&g,&NV_ENC_CODEC_H264_GUID,sizeof(g))==0;}),"GPU lacks H264 NVENC encoding");
        CUVIDPARSERPARAMS params{}; params.CodecType=cudaVideoCodec_HEVC;
        params.ulMaxNumDecodeSurfaces=1; params.ulClockRate=90000; params.ulMaxDisplayDelay=0;
        params.pUserData=this; params.pfnSequenceCallback=&sequence; params.pfnDecodePicture=&picture; params.pfnDisplayPicture=&display;
        cu(parser_create(&parser,&params),"cuvidCreateVideoParser");
    }
    // NVIDIA invokes these synchronously from ParseVideoData on this worker.
    // Never let a C++ exception unwind through a vendor C callback.
    template<class F> int callback(F fn) noexcept {
        try { return fn(); } catch (const std::exception& e) { callback_error=e.what(); return 0; }
        catch (...) { callback_error="Unexpected NVIDIA callback failure"; return 0; }
    }
    static int CUDAAPI sequence(void* user,CUVIDEOFORMAT* format) {
        auto& self=*static_cast<Engine*>(user); return self.callback([&]{return self.configure(format);});
    }
    static int CUDAAPI picture(void* user,CUVIDPICPARAMS* params) {
        auto& self=*static_cast<Engine*>(user); return self.callback([&]{cu(self.decode_picture(self.decoder,params),"cuvidDecodePicture");return 1;});
    }
    static int CUDAAPI display(void* user,CUVIDPARSERDISPINFO* info) {
        auto& self=*static_cast<Engine*>(user); return self.callback([&]{self.encode_frame(info);return 1;});
    }
    int configure(CUVIDEOFORMAT* f) {
        require(f->codec==cudaVideoCodec_HEVC && f->chroma_format==cudaVideoChromaFormat_420 &&
            f->bit_depth_luma_minus8==0 && f->bit_depth_chroma_minus8==0 && f->progressive_sequence,
            "Supported GPU input: progressive HEVC Main 8-bit 4:2:0 (Main10/interlaced unsupported)");
        unsigned w=f->display_area.right-f->display_area.left,h=f->display_area.bottom-f->display_area.top;
        require(w>=64 && h>=64 && w<=8192 && h<=8192 && !(w&1) && !(h&1),"Unsupported HEVC dimensions");
        if (initialized) {
            require(w==width && h==height && f->coded_width==coded_width && f->coded_height==coded_height && f->min_num_decode_surfaces<=surfaces,
                    "HEVC format changed; reconnect required");
            return static_cast<int>(surfaces);
        }
        width=w;height=h;coded_width=f->coded_width;coded_height=f->coded_height;
        surfaces=std::max(1u,static_cast<unsigned>(f->min_num_decode_surfaces));
        require(surfaces<=32,"HEVC requires too many decode surfaces");
        if (f->frame_rate.numerator && f->frame_rate.denominator) {fps_num=f->frame_rate.numerator;fps_den=f->frame_rate.denominator;}
        else {fps_num=90000;fps_den=rtp_duration;}
        require(static_cast<uint64_t>(fps_num)<=240ull*fps_den,"Unsupported HEVC frame rate");
        CUVIDDECODECREATEINFO d{}; d.CodecType=cudaVideoCodec_HEVC;d.ChromaFormat=cudaVideoChromaFormat_420;
        d.OutputFormat=cudaVideoSurfaceFormat_NV12;d.ulWidth=coded_width;d.ulHeight=coded_height;
        d.ulNumDecodeSurfaces=surfaces;d.ulNumOutputSurfaces=2;d.ulCreationFlags=cudaVideoCreate_PreferCUVID;
        d.DeinterlaceMode=cudaVideoDeinterlaceMode_Weave;d.ulTargetWidth=width;d.ulTargetHeight=height;
        d.display_area.left=f->display_area.left;d.display_area.top=f->display_area.top;
        d.display_area.right=f->display_area.right;d.display_area.bottom=f->display_area.bottom;
        d.target_rect.right=static_cast<short>(width);d.target_rect.bottom=static_cast<short>(height);
        cu(decoder_create(&decoder,&d),"cuvidCreateDecoder");
        NV_ENC_PRESET_CONFIG preset{};preset.version=NV_ENC_PRESET_CONFIG_VER;preset.presetCfg.version=NV_ENC_CONFIG_VER;
        nv(api.nvEncGetEncodePresetConfigEx(encoder,NV_ENC_CODEC_H264_GUID,NV_ENC_PRESET_P4_GUID,NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,&preset),"nvEncGetEncodePresetConfigEx");
        NV_ENC_CONFIG config=preset.presetCfg;
        config.profileGUID=NV_ENC_H264_PROFILE_BASELINE_GUID; config.gopLength=std::max(1u,(fps_num+fps_den-1)/fps_den);config.frameIntervalP=1;
        config.rcParams.rateControlMode=NV_ENC_PARAMS_RC_CBR;config.rcParams.averageBitRate=bitrate;config.rcParams.maxBitRate=bitrate;
        config.rcParams.enableLookahead=0;config.rcParams.zeroReorderDelay=1;
        config.rcParams.vbvBufferSize=std::max(1u,static_cast<unsigned>(static_cast<uint64_t>(bitrate)*fps_den/fps_num));
        config.rcParams.vbvInitialDelay=config.rcParams.vbvBufferSize;
        config.encodeCodecConfig.h264Config.repeatSPSPPS=1;config.encodeCodecConfig.h264Config.idrPeriod=config.gopLength;
        config.encodeCodecConfig.h264Config.entropyCodingMode=NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC;
        NV_ENC_INITIALIZE_PARAMS p{};p.version=NV_ENC_INITIALIZE_PARAMS_VER;p.encodeGUID=NV_ENC_CODEC_H264_GUID;p.presetGUID=NV_ENC_PRESET_P4_GUID;
        p.tuningInfo=NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;p.encodeWidth=width;p.encodeHeight=height;p.darWidth=width;p.darHeight=height;
        p.frameRateNum=fps_num;p.frameRateDen=fps_den;p.enablePTD=1;p.encodeConfig=&config;
        p.maxEncodeWidth=width;p.maxEncodeHeight=height;
        nv(api.nvEncInitializeEncoder(encoder,&p),"nvEncInitializeEncoder");
        NV_ENC_CREATE_BITSTREAM_BUFFER b{};b.version=NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        nv(api.nvEncCreateBitstreamBuffer(encoder,&b),"nvEncCreateBitstreamBuffer");bitstream=b.bitstreamBuffer;
        initialized=true;return static_cast<int>(surfaces);
    }
    void encode_frame(CUVIDPARSERDISPINFO* info) {
        require(info && initialized && info->progressive_frame,"Invalid decoded HEVC picture");
        CUdeviceptr address=0;unsigned pitch=0;
        NV_ENC_REGISTERED_PTR resource=nullptr;NV_ENC_INPUT_PTR input=nullptr;bool locked=false;
        auto cleanup=[&]{
            if (locked) api.nvEncUnlockBitstream(encoder,bitstream);
            if (input) api.nvEncUnmapInputResource(encoder,input);
            if (resource) api.nvEncUnregisterResource(encoder,resource);
            if (address) unmap_frame(decoder,address);
        };
        try {
            CUVIDPROCPARAMS process{};process.progressive_frame=1;
            cu(map_frame(decoder,info->picture_index,&address,&pitch,&process),"cuvidMapVideoFrame");
            NV_ENC_REGISTER_RESOURCE reg{};reg.version=NV_ENC_REGISTER_RESOURCE_VER;reg.resourceType=NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR;
            reg.width=width;reg.height=height;reg.pitch=pitch;reg.resourceToRegister=reinterpret_cast<void*>(address);
            reg.bufferFormat=NV_ENC_BUFFER_FORMAT_NV12;reg.bufferUsage=NV_ENC_INPUT_IMAGE;
            nv(api.nvEncRegisterResource(encoder,&reg),"nvEncRegisterResource");resource=reg.registeredResource;
            NV_ENC_MAP_INPUT_RESOURCE map{};map.version=NV_ENC_MAP_INPUT_RESOURCE_VER;map.registeredResource=resource;
            nv(api.nvEncMapInputResource(encoder,&map),"nvEncMapInputResource");input=map.mappedResource;
            NV_ENC_PIC_PARAMS pic{};pic.version=NV_ENC_PIC_PARAMS_VER;pic.inputBuffer=input;pic.bufferFmt=NV_ENC_BUFFER_FORMAT_NV12;
            pic.inputWidth=width;pic.inputHeight=height;pic.inputPitch=pitch;pic.outputBitstream=bitstream;
            pic.pictureStruct=NV_ENC_PIC_STRUCT_FRAME;pic.inputTimeStamp=static_cast<uint64_t>(info->timestamp);
            // With no B frames / no lookahead, each submission must produce one picture.
            nv(api.nvEncEncodePicture(encoder,&pic),"nvEncEncodePicture");
            NV_ENC_LOCK_BITSTREAM lock{};lock.version=NV_ENC_LOCK_BITSTREAM_VER;lock.outputBitstream=bitstream;
            nv(api.nvEncLockBitstream(encoder,&lock),"nvEncLockBitstream");locked=true;
            require(lock.bitstreamSizeInBytes>0 && lock.bitstreamSizeInBytes<=8*1024*1024 && output.size()<32,"NVENC output exceeds frame/queue limit");
            auto start=static_cast<unsigned char*>(lock.bitstreamBufferPtr);
            output.push_back({std::vector<unsigned char>(start,start+lock.bitstreamSizeInBytes),lock.outputTimeStamp});
            cleanup();
        } catch (...) {cleanup();throw;}
    }
    void push(const unsigned char* data,size_t size,uint64_t pts,unsigned duration) {
        require(size>0 && size<=8*1024*1024,"HEVC access unit exceeds limit");
        require(duration>0 && duration<=900000,"Invalid HEVC frame duration");rtp_duration=duration;
        cu(context_set(context),"cuCtxSetCurrent");
        CUVIDSOURCEDATAPACKET packet{};packet.flags=CUVID_PKT_TIMESTAMP|CUVID_PKT_ENDOFPICTURE;
        packet.payload=data;packet.payload_size=static_cast<unsigned long>(size);packet.timestamp=static_cast<CUvideotimestamp>(pts);
        auto status=parse(parser,&packet);
        if (!callback_error.empty()) throw std::runtime_error(callback_error);
        cu(status,"cuvidParseVideoData");
    }
};
}
extern "C" {
void* uvms_nv_create(unsigned device,unsigned bitrate,char* error,size_t size) noexcept {
    try {auto engine=std::make_unique<Engine>();engine->open(device,bitrate);return engine.release();}
    catch(const std::exception& e){error_text(error,size,e.what());return nullptr;}
    catch(...){error_text(error,size,"NVIDIA initialization failed");return nullptr;}
}
void uvms_nv_destroy(void* engine) noexcept {delete static_cast<Engine*>(engine);}
int uvms_nv_push(void* engine,const unsigned char* data,size_t length,uint64_t timestamp,unsigned duration,char* error,size_t size) noexcept {
    try {require(engine && data,"Invalid NVIDIA input");static_cast<Engine*>(engine)->push(data,length,timestamp,duration);return 0;}
    catch(const std::exception& e){error_text(error,size,e.what());return -1;}
    catch(...){error_text(error,size,"NVIDIA processing failed");return -1;}
}
// Borrowed packet remains valid until pop/destroy; Rust copies only compressed output.
int uvms_nv_peek(void* engine,const unsigned char** data,size_t* size,uint64_t* timestamp,unsigned* width,unsigned* height) noexcept {
    auto& e=*static_cast<Engine*>(engine);if(e.output.empty())return 0;
    auto& p=e.output.front();*data=p.bytes.data();*size=p.bytes.size();*timestamp=p.timestamp;*width=e.width;*height=e.height;return 1;
}
void uvms_nv_pop(void* engine) noexcept {static_cast<Engine*>(engine)->output.pop_front();}
}
