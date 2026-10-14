fn main() {
    println!("cargo:rerun-if-env-changed=CUDA_INCLUDE_DIR");
    println!("cargo:rerun-if-changed=bridge.cpp");
    println!("cargo:rerun-if-changed=include");
    if std::env::var_os("CARGO_FEATURE_NVIDIA").is_none() {
        return;
    }
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_OS").unwrap(),
        "linux",
        "NVIDIA bridge currently targets Linux"
    );
    let cuda =
        std::env::var("CUDA_INCLUDE_DIR").unwrap_or_else(|_| "/usr/local/cuda/include".into());
    assert!(
        std::path::Path::new(&cuda).join("cuda.h").exists(),
        "Install CUDA headers or run scripts/prepare-nvidia-headers.py and set CUDA_INCLUDE_DIR"
    );
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("bridge.cpp")
        .include("include")
        .include(cuda)
        .flag("-Werror=return-type")
        .compile("uvms_nvidia");
    println!("cargo:rustc-link-lib=dl");
}
