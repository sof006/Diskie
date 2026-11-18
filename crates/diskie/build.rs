//! Embeds the Windows resources: the icon and version information.
//!
//! The application manifest (DPI awareness, common controls) comes from GPUI, which embeds
//! its own. Diskie asks for administrator rights at startup instead (see `elevate.rs`), so a
//! declined prompt still leaves a working app.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=res/diskie.rc");
    println!("cargo:rerun-if-changed=res/diskie.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let res = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("res");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<&str> = version.split(['.', '-']).take(3).collect();
    parts.resize(3, "0");
    let commas = format!("{},{},{},0", parts[0], parts[1], parts[2]);

    let ico = res.join("diskie.ico").to_string_lossy().replace('\\', "\\\\");
    let rc = std::fs::read_to_string(res.join("diskie.rc"))
        .unwrap()
        .replace("\"diskie.ico\"", &format!("\"{ico}\""))
        .replace("0,1,0,0", &commas)
        .replace("\"0.1.0\"", &format!("\"{version}\""));
    let rc_path = out.join("diskie.rc");
    std::fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional().unwrap();
}
