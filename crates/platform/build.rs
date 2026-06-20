// The `screencapturekit` crate links Swift bridges (apple-cf / apple-metal) that reference
// `@rpath/libswift_Concurrency.dylib`. macOS keeps the Swift Concurrency runtime in the OS itself
// (resolved from the dyld shared cache via `/usr/lib/swift`, even though no file sits on disk there),
// so we embed that directory as an rpath — the shippable fix for the spike-A "no Swift rpath" finding
// (vs. the spike's dev-only `DYLD_LIBRARY_PATH` toolchain workaround). Emitted for this package's own
// test/example binaries; the `captured` binary carries the same rpath via the daemon's build.rs.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
