fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let runtime = std::path::Path::new(&manifest_dir).join("files/runtime");
    if runtime.exists() {
        println!("cargo:rustc-link-search={}", runtime.display());
        // Embed rpath so the dynamic linker finds libvosk.so at runtime
        // Binary is at target/debug/ → need 2 levels up to reach project root
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../files/runtime"
        );
    }
}
