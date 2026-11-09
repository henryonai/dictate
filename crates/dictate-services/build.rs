//! Linux only: the binaries of this crate (its tests) have to find the shared sherpa-onnx library, which
//! `sherpa-onnx-sys` copies into `target/<profile>`. Tests are built into `target/<profile>/deps`, hence `..`.
//! (A build script's link arguments reach only the binaries of its own package, so `sherpa-onnx-sys` cannot set this
//! up for the packages that use it.)

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
