//! Signs releases so the app's auto-updater will accept them.
//!
//!   release-tool keygen               create the signing key (once)
//!   release-tool sign <exe> <version>  write <exe>.sig
//!
//! The private key lives outside the repository, in
//! ~/.config/ram-optimizer/update-signing.key. Back it up: without it, new
//! releases can't be signed and installed copies won't update.

use std::fs;
use std::path::PathBuf;

use ram_core::update;

const PUBLIC_KEY_FILE: &str = "crates/app/update-key.pub";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen"] => keygen(),
        ["sign", exe, version] => sign(exe, version),
        _ => Err("usage: release-tool keygen | release-tool sign <exe> <version>".into()),
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn key_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME isn't set")?;
    Ok(PathBuf::from(home).join(".config/ram-optimizer/update-signing.key"))
}

fn keygen() -> Result<(), String> {
    let path = key_path()?;
    if path.exists() {
        return Err(format!("{} already exists; refusing to replace it", path.display()));
    }
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| e.to_string())?;
    let secret_hex = update::to_hex(&secret);
    let public_hex = update::public_key(&secret_hex)?;

    fs::create_dir_all(path.parent().ok_or("bad key path")?).map_err(|e| e.to_string())?;
    fs::write(&path, &secret_hex).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    fs::write(PUBLIC_KEY_FILE, format!("{public_hex}\n")).map_err(|e| e.to_string())?;
    println!("Private key: {} (keep it secret, back it up)", path.display());
    println!("Public key:  {PUBLIC_KEY_FILE}");
    Ok(())
}

fn sign(exe: &str, version: &str) -> Result<(), String> {
    let secret = fs::read_to_string(key_path()?).map_err(|e| format!("can't read signing key: {e}"))?;
    let bytes = fs::read(exe).map_err(|e| format!("can't read {exe}: {e}"))?;
    let signature = update::sign(&secret, version, &bytes)?;

    // Check against the key the app is built with, to catch a mismatch now
    // rather than after users stop receiving updates.
    let public = fs::read_to_string(PUBLIC_KEY_FILE).map_err(|e| e.to_string())?;
    update::verify(&public, version, &bytes, &signature)
        .map_err(|_| format!("signing key doesn't match {PUBLIC_KEY_FILE}"))?;

    let sig_path = format!("{exe}.sig");
    fs::write(&sig_path, format!("{signature}\n")).map_err(|e| e.to_string())?;
    println!("Signed {exe} as version {version} -> {sig_path}");
    Ok(())
}
