fn main() {
    // `coreaudio-sys`'s `core_audio` feature links `CoreAudio.framework`
    // itself; `device.rs` also calls straight CoreFoundation (CFString),
    // which needs its own explicit link.
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
