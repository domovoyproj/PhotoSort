fn main() {
    println!("cargo:rerun-if-changed=packaging/photosort.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("packaging/photosort.ico")
        .set("ProductName", "PhotoSort")
        .set("FileDescription", "PhotoSort")
        .set("OriginalFilename", "PhotoSort.exe");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let toolkit = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join(".photosort/tools/mingw/mingw64/bin");
        if toolkit.join("windres.exe").is_file() {
            resource.set_toolkit_path(&toolkit.to_string_lossy());
        }
    }
    resource
        .compile()
        .expect("Failed to embed the PhotoSort icon");
}
