// Links the prebuilt Windows resources (exe icon + version info).
// Regenerate with build-resources.ps1 after changing assets/app.rc or the icon.
fn main() {
    let obj = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("resources.obj");
    println!("cargo:rerun-if-changed={}", obj.display());
    if obj.exists() && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    }
}
