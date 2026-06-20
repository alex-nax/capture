// capture-engine links capture-platform → screencapturekit → the Swift bridges, which reference
// `@rpath/libswift_Concurrency.dylib`. Embed `/usr/lib/swift` (the OS Swift runtime in the dyld
// shared cache) as an rpath so this crate's test/example binaries load — build-script link args from
// capture-platform don't propagate to dependent binaries. macOS only. See capture-platform/build.rs.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
