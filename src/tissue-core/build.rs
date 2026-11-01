// #143: expose the compile-time target triple to the crate so
// GET /api/meta can report build.target without hand-maintained values.
fn main() {
    println!(
        "cargo:rustc-env=TISSUE_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
