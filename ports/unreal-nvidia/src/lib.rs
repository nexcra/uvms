//! A thread-confined, direct NVIDIA SDK adapter. Driver libraries load only on demand.
use anyhow::{bail, Result};

pub const COMPILED: bool = cfg!(feature = "nvidia");
pub struct Encoded {
    pub annex_b: Vec<u8>,
    pub timestamp_90k: u64,
    pub width: u32,
    pub height: u32,
}
#[cfg(feature = "nvidia")]
mod ffi {
    use std::ffi::{c_char, c_void};
    extern "C" {
        pub fn uvms_nv_create(
            device: u32,
            bitrate: u32,
            error: *mut c_char,
            size: usize,
        ) -> *mut c_void;
        pub fn uvms_nv_destroy(engine: *mut c_void);
        pub fn uvms_nv_push(
            engine: *mut c_void,
            data: *const u8,
            length: usize,
            timestamp: u64,
            duration: u32,
            error: *mut c_char,
            size: usize,
        ) -> i32;
        pub fn uvms_nv_peek(
            engine: *mut c_void,
            data: *mut *const u8,
            size: *mut usize,
            timestamp: *mut u64,
            width: *mut u32,
            height: *mut u32,
        ) -> i32;
        pub fn uvms_nv_pop(engine: *mut c_void);
    }
}
pub struct Transcoder {
    #[cfg(feature = "nvidia")]
    engine: std::ptr::NonNull<std::ffi::c_void>,
    // CUDA context and callbacks stay on the owning OS thread.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Transcoder {
    pub fn new(device: u32, bitrate: u32) -> Result<Self> {
        if device > 63 || !(100_000..=100_000_000).contains(&bitrate) {
            bail!("Invalid NVIDIA device or H264 bitrate")
        }
        #[cfg(feature = "nvidia")]
        {
            let mut error = [0i8; 512];
            let raw =
                unsafe { ffi::uvms_nv_create(device, bitrate, error.as_mut_ptr(), error.len()) };
            let engine = std::ptr::NonNull::new(raw).ok_or_else(|| {
                anyhow::anyhow!(unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned())
            })?;
            Ok(Self {
                engine,
                _thread: std::marker::PhantomData,
            })
        }
        #[cfg(not(feature = "nvidia"))]
        bail!("H265 requires a build with --features nvidia and an NVIDIA NVDEC/NVENC GPU")
    }
    pub fn push(
        &mut self,
        annex_b: &[u8],
        timestamp_90k: u64,
        duration_90k: u32,
    ) -> Result<Vec<Encoded>> {
        if annex_b.is_empty() || annex_b.len() > 8 * 1024 * 1024 {
            bail!("HEVC access unit exceeds limit")
        }
        #[cfg(feature = "nvidia")]
        {
            let mut error = [0i8; 512];
            if unsafe {
                ffi::uvms_nv_push(
                    self.engine.as_ptr(),
                    annex_b.as_ptr(),
                    annex_b.len(),
                    timestamp_90k,
                    duration_90k,
                    error.as_mut_ptr(),
                    error.len(),
                )
            } != 0
            {
                bail!(unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) }
                    .to_string_lossy()
                    .into_owned());
            }
            let mut output = Vec::new();
            loop {
                let (mut data, mut size, mut pts, mut width, mut height) =
                    (std::ptr::null(), 0, 0, 0, 0);
                if unsafe {
                    ffi::uvms_nv_peek(
                        self.engine.as_ptr(),
                        &mut data,
                        &mut size,
                        &mut pts,
                        &mut width,
                        &mut height,
                    )
                } == 0
                {
                    break;
                }
                anyhow::ensure!(
                    !data.is_null() && size <= 8 * 1024 * 1024 && output.len() < 32,
                    "Invalid NVIDIA output"
                );
                // The bridge owns this span until pop; only the compressed bytes cross to host Rust.
                let annex_b = unsafe { std::slice::from_raw_parts(data, size) }.to_vec();
                unsafe { ffi::uvms_nv_pop(self.engine.as_ptr()) };
                output.push(Encoded {
                    annex_b,
                    timestamp_90k: pts,
                    width,
                    height,
                });
            }
            Ok(output)
        }
        #[cfg(not(feature = "nvidia"))]
        {
            let _ = (timestamp_90k, duration_90k);
            bail!("NVIDIA support is not compiled")
        }
    }
}
#[cfg(feature = "nvidia")]
impl Drop for Transcoder {
    fn drop(&mut self) {
        unsafe { ffi::uvms_nv_destroy(self.engine.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_settings_before_opening_driver() {
        assert!(Transcoder::new(64, 4_000_000).is_err());
        assert!(Transcoder::new(0, 0).is_err());
    }
}
