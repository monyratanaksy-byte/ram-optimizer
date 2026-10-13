fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");

    // Icon, version info and manifest for the Windows .exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=app.manifest");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed Windows resources");
    }
}
