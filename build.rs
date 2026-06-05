fn main() {
    let runtime = std::path::Path::new("files/runtime");
    if runtime.exists() {
        println!(
            "cargo:rustc-link-search={}",
            std::env::current_dir().unwrap().join(runtime).display()
        );
    }
}
