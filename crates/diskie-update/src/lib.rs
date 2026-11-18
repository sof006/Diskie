//! Keeping Diskie current from its GitHub Releases.
//!
//! 1. [`check`] downloads `latest.json` from the latest release. That is a plain file download
//!    (`/releases/latest/download/…`), not the GitHub API, so there is no rate limit or token,
//!    and nothing is sent beyond an ordinary HTTP request.
//! 2. [`download`] fetches this platform's file and verifies it before anything else happens:
//!    its SHA-256 must match the manifest, and its minisign signature must verify against
//!    [`PUBLIC_KEY`], which is compiled in. The signature's trusted comment names the version
//!    and file, so an old, genuinely signed build can't be passed off as a new one.
//! 3. [`install`] keeps a copy of the running executable as `Diskie.old.exe` and swaps the new
//!    one in (a running `.exe` can be renamed but not overwritten). [`on_startup`] rolls back
//!    if the new version fails to start cleanly twice; [`confirm_healthy`] clears that state.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use semver::Version as SemVer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_URL: &str = "https://github.com/sof006/Diskie/releases/latest/download/latest.json";
pub const RELEASES_PAGE: &str = "https://github.com/sof006/Diskie/releases";

/// The release-signing public key (minisign). Its secret half lives only in the repository's
/// GitHub Actions secrets and the maintainer's offline backup.
pub const PUBLIC_KEY: &str = include_str!("../../../keys/diskie-release.pub");

const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// What `latest.json` holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Release notes (Markdown, from the GitHub release text).
    #[serde(default)]
    pub notes: String,
    /// RFC 3339 publish time.
    #[serde(default)]
    pub published: String,
    /// Keyed by [`platform`], for example `windows-x86_64`.
    pub assets: std::collections::BTreeMap<String, Asset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// The complete `.minisig` file contents.
    pub signature: String,
}

#[derive(Clone, Debug)]
pub struct Update {
    pub version: semver::Version,
    pub notes: String,
    pub asset: Asset,
}

#[derive(Debug)]
pub enum Error {
    Network(String),
    Manifest(String),
    /// The download did not match its checksum or signature. Nothing was changed.
    Verification(String),
    Install(String),
    /// This build can't update itself (for example a distribution package).
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(e) => write!(f, "could not reach GitHub: {e}"),
            Error::Manifest(e) => write!(f, "the update information is not valid: {e}"),
            Error::Verification(e) => write!(f, "the download failed verification, so it was not installed: {e}"),
            Error::Install(e) => write!(f, "installing the update failed: {e}"),
            Error::Unsupported(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

/// The asset key for this build.
pub fn platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-aarch64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else {
        "unknown"
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(concat!("Diskie/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Looks for a newer release. `Ok(None)` when `current` is the newest.
pub fn check(current: &str) -> Result<Option<Update>, Error> {
    let current = semver::Version::parse(current).map_err(|e| Error::Manifest(e.to_string()))?;
    let mut resp = match agent().get(manifest_url()).call() {
        Ok(r) => r,
        // No release has a manifest yet: nothing to update to.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(Error::Network(e.to_string())),
    };
    let body = resp
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::Network(e.to_string()))?;
    evaluate(&body, &current)
}

/// Debug builds can point at a local test server with `DISKIE_UPDATE_MANIFEST_URL`. Release
/// builds always use GitHub.
fn manifest_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(u) = std::env::var("DISKIE_UPDATE_MANIFEST_URL") {
        return u;
    }
    MANIFEST_URL.to_string()
}

fn test_server(url: &str) -> bool {
    cfg!(debug_assertions) && url.starts_with("http://127.0.0.1:")
}

/// The decision part of [`check`], separate so it can be tested without a network.
pub fn evaluate(manifest_json: &str, current: &semver::Version) -> Result<Option<Update>, Error> {
    let m: Manifest = serde_json::from_str(manifest_json).map_err(|e| Error::Manifest(e.to_string()))?;
    let version = semver::Version::parse(m.version.trim_start_matches('v')).map_err(|e| Error::Manifest(e.to_string()))?;
    if version <= *current {
        return Ok(None);
    }
    // A pre-release only reaches people already running a pre-release.
    if !version.pre.is_empty() && current.pre.is_empty() {
        return Ok(None);
    }
    let asset = m.assets.get(platform()).cloned().ok_or_else(|| Error::Manifest(format!("no download for {}", platform())))?;
    Ok(Some(Update { version, notes: m.notes, asset }))
}

/// Downloads the update into a temporary file and verifies it. Returns the verified file.
/// `on_progress` gets (bytes so far, total).
pub fn download(update: &Update, mut on_progress: impl FnMut(u64, u64)) -> Result<PathBuf, Error> {
    let a = &update.asset;
    if !a.url.starts_with("https://github.com/") && !a.url.starts_with("https://objects.githubusercontent.com/") && !test_server(&a.url) {
        return Err(Error::Manifest(format!("unexpected download location {}", a.url)));
    }
    let mut resp = agent().get(&a.url).call().map_err(|e| Error::Network(e.to_string()))?;
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut data = Vec::with_capacity(a.size as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| Error::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        on_progress(data.len() as u64, a.size);
    }
    verify(&data, a, &update.version)?;

    let path = std::env::temp_dir().join(format!("Diskie-{}-{}", update.version, a.name));
    std::fs::write(&path, &data).map_err(|e| Error::Install(e.to_string()))?;
    Ok(path)
}

/// Checks size, SHA-256 and the minisign signature (including its trusted comment).
pub fn verify(data: &[u8], a: &Asset, version: &semver::Version) -> Result<(), Error> {
    verify_with_key(data, a, version, PUBLIC_KEY)
}

fn verify_with_key(data: &[u8], a: &Asset, version: &semver::Version, public_key: &str) -> Result<(), Error> {
    if data.len() as u64 != a.size {
        return Err(Error::Verification(format!("expected {} bytes, got {}", a.size, data.len())));
    }
    let digest = hex(&Sha256::digest(data));
    if !digest.eq_ignore_ascii_case(a.sha256.trim()) {
        return Err(Error::Verification("checksum mismatch".into()));
    }
    let pk = minisign_verify::PublicKey::from_base64(public_key.trim()).map_err(|e| Error::Verification(format!("bad public key: {e}")))?;
    let sig = minisign_verify::Signature::decode(&a.signature).map_err(|e| Error::Verification(format!("bad signature: {e}")))?;
    pk.verify(data, &sig, false).map_err(|e| Error::Verification(format!("signature does not verify: {e}")))?;
    let expected = trusted_comment(&version.to_string(), &a.name);
    if sig.trusted_comment() != expected {
        return Err(Error::Verification(format!("signature is for \"{}\", not \"{expected}\"", sig.trusted_comment())));
    }
    Ok(())
}

/// The trusted comment the release tooling signs into every signature.
pub fn trusted_comment(version: &str, file_name: &str) -> String {
    format!("diskie {version} {file_name}")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Installing and rolling back

#[derive(Serialize, Deserialize, Default)]
struct PendingState {
    from: String,
    to: String,
    starts: u32,
}

fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("XDG_STATE_HOME")).map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state"))
    })?;
    Some(base.join("Diskie").join("update-state.json"))
}

fn backup_path(exe: &Path) -> PathBuf {
    let ext = exe.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    exe.with_file_name(format!("Diskie.old{ext}"))
}

/// Whether this copy can replace itself. False when the executable's folder is not writable.
pub fn can_self_update() -> Result<(), Error> {
    if std::env::var_os("DISKIE_NO_SELF_UPDATE").is_some() {
        return Err(Error::Unsupported("updates are managed by the package that installed Diskie"));
    }
    let exe = current_exe().map_err(|e| Error::Install(e.to_string()))?;
    let dir = exe.parent().ok_or(Error::Unsupported("cannot tell where Diskie is installed"))?;
    let probe = dir.join(".diskie-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(probe);
            Ok(())
        }
        Err(_) => Err(Error::Unsupported("Diskie's folder is not writable, so it cannot update itself")),
    }
}

/// Swaps the verified download in for the running executable. Takes effect on the next start.
///
/// Returns the path to start the new version from. That is the same path, unless the file still
/// had the old release's name (`Diskie-0.1.0-windows-x64.exe`): then it is renamed to match the
/// new version. Windows keeps reporting a running program's original path, so restart from the
/// returned path (or [`current_exe`]), not `std::env::current_exe()`.
pub fn install(downloaded: &Path, from_version: &str, to_version: &str) -> Result<PathBuf, Error> {
    can_self_update()?;
    let exe = current_exe().map_err(|e| Error::Install(e.to_string()))?;
    std::fs::copy(&exe, backup_path(&exe)).map_err(|e| Error::Install(format!("keeping the old version: {e}")))?;
    self_replace::self_replace(downloaded).map_err(|e| Error::Install(e.to_string()))?;
    let _ = std::fs::remove_file(downloaded);
    if let Some(p) = state_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let s = PendingState { from: from_version.into(), to: to_version.into(), starts: 0 };
        let _ = std::fs::write(p, serde_json::to_vec(&s).unwrap_or_default());
    }
    Ok(rename_for_version(&exe, to_version))
}

/// The file name a version is published under: `Diskie-0.1.1-windows-x64.exe`.
pub fn release_file_name(version: &str) -> String {
    format!("Diskie-{version}-windows-x64.exe")
}

/// Set when Diskie renames its own executable while running.
static RENAMED_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where this executable is now. Like `std::env::current_exe()`, but it follows a rename Diskie
/// made itself: Windows keeps reporting the path a program was started from.
pub fn current_exe() -> std::io::Result<PathBuf> {
    if let Some(p) = RENAMED_EXE.lock().ok().and_then(|g| g.clone()) {
        return Ok(p);
    }
    std::env::current_exe()
}

/// Renames the running executable `exe` to `version`'s release file name, if it carries some
/// release's file name now (`Diskie-0.1.0-windows-x64.exe`). Leaves any other name alone (a copy
/// renamed to `Diskie.exe` stays that way), and never replaces a file already using the new name.
/// Returns the path the executable now has.
fn rename_for_version(exe: &Path, version: &str) -> PathBuf {
    let Some(new) = renamed_for_version(exe, version) else { return exe.to_path_buf() };
    if new.exists() || std::fs::rename(exe, &new).is_err() {
        return exe.to_path_buf();
    }
    if let Ok(mut g) = RENAMED_EXE.lock() {
        *g = Some(new.clone());
    }
    new
}

fn renamed_for_version(exe: &Path, version: &str) -> Option<PathBuf> {
    let name = exe.file_name()?.to_str()?;
    let wanted = release_file_name(version);
    (is_release_file_name(name) && !name.eq_ignore_ascii_case(&wanted)).then(|| exe.with_file_name(wanted))
}

/// `Diskie-<version>-windows-x64.exe`, any version, any case.
fn is_release_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_prefix("diskie-")
        .and_then(|rest| rest.strip_suffix("-windows-x64.exe"))
        .is_some_and(|v| semver::Version::parse(v).is_ok())
}

pub enum Startup {
    Normal,
    /// The first start after an update.
    Updated { from: String },
    /// The update failed to start twice; the previous version was put back and should be started
    /// from `exe` (renamed back when the update had renamed it).
    RolledBack { failed: String, exe: PathBuf },
}

/// Call first thing at startup with this build's version.
pub fn on_startup(current: &str) -> Startup {
    let Some(path) = state_path() else { return Startup::Normal };
    let Ok(bytes) = std::fs::read(&path) else { return Startup::Normal };
    let Ok(mut s) = serde_json::from_slice::<PendingState>(&bytes) else {
        let _ = std::fs::remove_file(&path);
        return Startup::Normal;
    };
    if s.to != current {
        // Running something else (rolled back by hand, or an older copy): forget the update.
        let _ = std::fs::remove_file(&path);
        return Startup::Normal;
    }
    s.starts += 1;
    if s.starts > 2
        && let Ok(exe) = current_exe()
    {
        let old = backup_path(&exe);
        if old.exists() && self_replace::self_replace(&old).is_ok() {
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::remove_file(&path);
            let exe = rename_for_version(&exe, &s.from);
            return Startup::RolledBack { failed: s.to, exe };
        }
    }
    let _ = std::fs::write(&path, serde_json::to_vec(&s).unwrap_or_default());
    // Versions before 0.1.2 didn't rename the file when they installed an update, so it can still
    // carry an older release's name. Put that right now (the restarted copy is this version).
    if s.starts == 1
        && let Ok(exe) = current_exe()
    {
        rename_for_version(&exe, current);
    }
    Startup::Updated { from: s.from }
}

/// Call once the app has been running normally for a little while after an update.
pub fn confirm_healthy() {
    if let Some(path) = state_path()
        && path.exists()
    {
        let _ = std::fs::remove_file(path);
        if let Ok(exe) = current_exe() {
            let _ = std::fs::remove_file(backup_path(&exe));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key pair, generated for these tests only (not the release key).
    const TEST_PK: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";

    fn asset(data: &[u8], signature: &str) -> Asset {
        Asset { name: "Diskie.exe".into(), url: "https://github.com/x".into(), size: data.len() as u64, sha256: hex(&Sha256::digest(data)), signature: signature.into() }
    }

    #[test]
    fn newer_versions_only() {
        let m = r#"{"version":"0.2.0","assets":{"windows-x86_64":{"name":"a","url":"u","size":1,"sha256":"x","signature":"s"},"linux-x86_64":{"name":"a","url":"u","size":1,"sha256":"x","signature":"s"}}}"#;
        let v = |s| semver::Version::parse(s).unwrap();
        assert!(evaluate(m, &v("0.1.0")).unwrap().is_some());
        assert!(evaluate(m, &v("0.2.0")).unwrap().is_none());
        assert!(evaluate(m, &v("0.3.0")).unwrap().is_none());
        let pre = m.replace("0.2.0", "0.2.0-beta.1");
        assert!(evaluate(&pre, &v("0.1.0")).unwrap().is_none());
        assert!(evaluate(&pre, &v("0.2.0-alpha.1")).unwrap().is_some());
    }

    #[test]
    fn accepts_a_genuine_signature_and_nothing_else() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let pk = pk_box.lines().nth(1).unwrap();
        let data = b"pretend this is Diskie 0.2.0";
        let v = semver::Version::new(0, 2, 0);
        let sign = |comment: &str| {
            minisign::sign(None, &kp.sk, std::io::Cursor::new(&data[..]), Some(comment), None).unwrap().to_string()
        };
        let good = asset(data, &sign(&trusted_comment("0.2.0", "Diskie.exe")));
        verify_with_key(data, &good, &v, pk).expect("genuine signature verifies");

        // Same file, signed honestly, but claimed to be a different version: rejected.
        assert!(verify_with_key(data, &good, &semver::Version::new(0, 3, 0), pk).is_err());
        let replay = asset(data, &sign(&trusted_comment("0.1.0", "Diskie.exe")));
        assert!(verify_with_key(data, &replay, &v, pk).is_err());

        // Changed bytes with a recomputed checksum: the signature still catches it.
        let tampered = b"pretend this is Diskie 0.2.1";
        let mut a = good.clone();
        a.sha256 = hex(&Sha256::digest(tampered));
        assert!(verify_with_key(tampered, &a, &v, pk).is_err());

        // The embedded release key is well-formed.
        assert!(minisign_verify::PublicKey::from_base64(PUBLIC_KEY.trim()).is_ok());
    }

    #[test]
    fn rejects_tampered_data() {
        let data = b"not really an executable";
        let a = asset(data, "untrusted comment: x\nRUQf6LRCGA9i5wAAAAAAAAAA\ntrusted comment: x\nAAAA\n");
        let r = verify_with_key(b"not really an executablE", &a, &semver::Version::new(0, 2, 0), TEST_PK);
        assert!(matches!(r, Err(Error::Verification(_))));
        let r = verify_with_key(data, &a, &semver::Version::new(0, 2, 0), TEST_PK);
        assert!(matches!(r, Err(Error::Verification(_))), "a garbage signature must not verify");
    }

    #[test]
    fn only_the_release_name_is_renamed() {
        let dir = Path::new("downloads");
        let renamed = |name: &str| renamed_for_version(&dir.join(name), "0.1.2");
        let new = Some(dir.join("Diskie-0.1.2-windows-x64.exe"));
        assert_eq!(renamed("Diskie-0.1.1-windows-x64.exe"), new);
        // An older name that an update before 0.1.2 left alone.
        assert_eq!(renamed("Diskie-0.1.0-windows-x64.exe"), new);
        assert_eq!(renamed("diskie-0.1.0-WINDOWS-x64.EXE"), new);
        assert_eq!(renamed("Diskie-0.1.2-windows-x64.exe"), None);
        assert_eq!(renamed("Diskie.exe"), None);
        assert_eq!(renamed("Diskie-latest-windows-x64.exe"), None);
        assert_eq!(renamed("Diskie-0.1.0-windows-x64 (1).exe"), None);
    }

    #[test]
    fn renaming_never_replaces_another_file() {
        let dir = std::env::temp_dir().join(format!("diskie-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join(release_file_name("0.1.0"));
        std::fs::write(&old, b"new build").unwrap();
        assert_eq!(rename_for_version(&old, "0.1.1"), dir.join(release_file_name("0.1.1")));
        assert!(!old.exists());

        // A manual download of 0.1.2 already sits there: keep it, and keep this file's name.
        let current = dir.join(release_file_name("0.1.1"));
        let other = dir.join(release_file_name("0.1.2"));
        std::fs::write(&other, b"someone else's file").unwrap();
        assert_eq!(rename_for_version(&current, "0.1.2"), current);
        assert_eq!(std::fs::read(&other).unwrap(), b"someone else's file");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
