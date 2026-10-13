use std::path::Path;

fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");

    // Icon, version info and manifest for the Windows .exe, stamped with the
    // package version so Explorer's Properties > Details shows the real one.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=app.rc.in");
        println!("cargo:rerun-if-changed=app.manifest.in");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        // The version lives in the workspace Cargo.toml.
        println!("cargo:rerun-if-changed=../../Cargo.toml");

        let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
        let numbers = format!("{},0", version.split('.').collect::<Vec<_>>().join(","));
        let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
        let manifest = Path::new(&out_dir).join("app.manifest");
        let rc = Path::new(&out_dir).join("app.rc");

        let fill = |template: &str| {
            std::fs::read_to_string(Path::new(&crate_dir).join(template))
                .expect("missing resource template")
                .replace("{{VERSION_NUMBERS}}", &numbers)
                .replace("{{VERSION}}", &version)
                .replace("{{ICON}}", &forward_slashes(&Path::new(&crate_dir).join("assets/icon.ico")))
                .replace("{{MANIFEST}}", &forward_slashes(&manifest))
        };
        std::fs::write(&manifest, fill("app.manifest.in")).expect("can't write manifest");
        std::fs::write(&rc, fill("app.rc.in")).expect("can't write resource file");

        embed_resource::compile(&rc, embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed Windows resources");
    }
}

/// Resource compilers accept forward slashes on every platform.
fn forward_slashes(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
