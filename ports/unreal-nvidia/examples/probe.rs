fn main() {
    match unreal_nvidia::Transcoder::new(0,4_000_000) {
        Ok(_) => println!("NVIDIA driver, HEVC 8-bit decode capability and H264 encode session available; media conversion still requires a hardware stream test"),
        Err(e) => {eprintln!("{e}");std::process::exit(2);}
    }
}
