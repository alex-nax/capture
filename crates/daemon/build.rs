// `captured` links capture-platform → screencapturekit → the Swift bridges, which reference
// `@rpath/libswift_Concurrency.dylib`. Embed `/usr/lib/swift` (the OS Swift runtime, resolved from
// the dyld shared cache) as an rpath so the binary loads — build-script link args don't propagate
// from capture-platform to this dependent binary, so it has to be re-emitted here. macOS only.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
