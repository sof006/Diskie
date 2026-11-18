//! Release tooling, run with `cargo xtask <command>`.
//!
//! ```text
//! cargo xtask keygen <dir>
//!     Writes a new minisign key pair: <dir>/diskie-release.key (secret, keep it offline and in
//!     the DISKIE_SIGNING_KEY Actions secret) and <dir>/diskie-release.pub.
//!
//! cargo xtask release <version> <exe> <out-dir> [notes-file]
//!     Copies the executable to <out-dir>/Diskie-<version>-windows-x64.exe, signs it with the
//!     key in DISKIE_SIGNING_KEY (the key file's contents) or DISKIE_SIGNING_KEY_FILE, and
//!     writes the .minisig, a SHA256SUMS file and latest.json.
//! ```

use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("keygen") if args.len() == 2 => keygen(Path::new(&args[1])),
        Some("release") if (4..=5).contains(&args.len()) => {
            release(&args[1], Path::new(&args[2]), Path::new(&args[3]), args.get(4).map(PathBuf::from))
        }
        _ => Err("usage: cargo xtask keygen <dir> | cargo xtask release <version> <exe> <out-dir> [notes-file]".into()),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

type Result<T> = std::result::Result<T, String>;

fn keygen(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sk_path = dir.join("diskie-release.key");
    if sk_path.exists() {
        return Err(format!("{} already exists; refusing to overwrite a signing key", sk_path.display()));
    }
    // An empty password stores the key unencrypted but with its checksum, which loading checks.
    let kp = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).map_err(|e| e.to_string())?;
    let pk = kp.pk.to_box().map_err(|e| e.to_string())?.to_string();
    let sk = kp.sk.to_box(None).map_err(|e| e.to_string())?.to_string();
    std::fs::write(&sk_path, sk).map_err(|e| e.to_string())?;
    // The app embeds only the base64 key line.
    let pk_line = pk.lines().nth(1).ok_or("unexpected public key format")?;
    std::fs::write(dir.join("diskie-release.pub"), format!("{pk_line}\n")).map_err(|e| e.to_string())?;
    println!("wrote {} and diskie-release.pub", sk_path.display());
    Ok(())
}

fn load_secret_key() -> Result<minisign::SecretKey> {
    let text = match std::env::var("DISKIE_SIGNING_KEY") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            let p = std::env::var("DISKIE_SIGNING_KEY_FILE").map_err(|_| "set DISKIE_SIGNING_KEY or DISKIE_SIGNING_KEY_FILE")?;
            std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?
        }
    };
    let sk_box = minisign::SecretKeyBox::from_string(text.trim()).map_err(|e| e.to_string())?;
    sk_box.into_secret_key(Some(String::new())).map_err(|e| e.to_string())
}

fn release(version: &str, exe: &Path, out: &Path, notes: Option<PathBuf>) -> Result<()> {
    let version = version.trim_start_matches('v');
    let workspace_version = env!("CARGO_PKG_VERSION");
    if version != workspace_version {
        return Err(format!("tag says {version} but the workspace version is {workspace_version}"));
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let name = diskie_update::release_file_name(version);
    let data = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    std::fs::write(out.join(&name), &data).map_err(|e| e.to_string())?;

    let sk = load_secret_key()?;
    let pk = minisign::PublicKey::from_base64(diskie_update::PUBLIC_KEY.trim()).map_err(|e| e.to_string())?;
    let trusted = diskie_update::trusted_comment(version, &name);
    let sig = minisign::sign(Some(&pk), &sk, Cursor::new(&data), Some(&trusted), Some("signed by the Diskie release workflow"))
        .map_err(|e| format!("signing failed (is the key the one matching keys/diskie-release.pub?): {e}"))?
        .to_string();
    std::fs::write(out.join(format!("{name}.minisig")), &sig).map_err(|e| e.to_string())?;

    let sha = diskie_update::hex(&Sha256::digest(&data));
    std::fs::write(out.join("SHA256SUMS"), format!("{sha}  {name}\n")).map_err(|e| e.to_string())?;

    let asset = diskie_update::Asset {
        name: name.clone(),
        url: format!("https://github.com/sof006/Diskie/releases/download/v{version}/{name}"),
        size: data.len() as u64,
        sha256: sha,
        signature: sig,
    };
    // Prove the round trip before publishing anything.
    diskie_update::verify(&data, &asset, &semver_parse(version)?).map_err(|e| format!("self-check failed: {e}"))?;

    let notes = match notes {
        Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => String::new(),
    };
    let manifest = diskie_update::Manifest {
        version: version.into(),
        notes,
        published: std::env::var("DISKIE_RELEASE_DATE").unwrap_or_default(),
        assets: [(diskie_update::platform().to_string(), asset)].into_iter().collect(),
    };
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(out.join("latest.json"), json).map_err(|e| e.to_string())?;
    println!("release files for {version} are in {}", out.display());
    Ok(())
}

fn semver_parse(v: &str) -> Result<diskie_update::SemVer> {
    diskie_update::SemVer::parse(v).map_err(|e| e.to_string())
}
