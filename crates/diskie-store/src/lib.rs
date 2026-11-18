//! Saved scans ("snapshots"), so a later scan of the same place can show what changed.
//!
//! Each finished scan is saved as one file, `<unix seconds>.snap`, in a folder per scanned
//! place under the user's local data folder (`%LOCALAPPDATA%\Diskie\snapshots` on Windows,
//! `~/.local/share/diskie/snapshots` on Linux). A file is a small JSON header, readable without
//! the rest, followed by the zstd-compressed tree. Only the newest few per place are kept.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use diskie_model::Tree;
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"DSKSNAP1";

/// How many snapshots to keep per scanned place.
pub const KEEP: usize = 5;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Info {
    /// The scanned folder, drive or share, as the user gave it.
    pub root: String,
    /// When the scan finished, Unix seconds.
    pub taken: u64,
    pub size: u64,
    pub files: u64,
    pub folders: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

/// Where snapshots live, or `None` when there is no home or app-data folder.
pub fn base_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Diskie").join("snapshots"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
            .map(|d| d.join("diskie").join("snapshots"))
    }
}

/// The folder for one scanned place: a readable name plus a hash, so `C:\` and `C:\Users`
/// never share one, and on Windows `c:\users` and `C:\Users` do.
fn place_dir(base: &Path, root: &str) -> PathBuf {
    let key = if cfg!(windows) { root.to_lowercase() } else { root.to_string() };
    let key = key.trim_end_matches(['\\', '/']);
    // FNV-1a: stable across versions, unlike std's hasher.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let readable: String = key.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect::<String>().trim_matches('_').chars().take(40).collect();
    base.join(format!("{readable}-{:08x}", hash as u32))
}

/// Saves `tree`, scanned from `root` and finished at `taken`, then deletes all but the newest
/// [`KEEP`] snapshots of that place.
pub fn save(base: &Path, root: &str, tree: &Tree, taken: u64) -> io::Result<PathBuf> {
    let dir = place_dir(base, root);
    std::fs::create_dir_all(&dir)?;
    let info = Info {
        root: root.to_string(),
        taken,
        size: tree.size[Tree::ROOT as usize],
        files: tree.files[Tree::ROOT as usize] as u64,
        folders: tree.dirs[Tree::ROOT as usize] as u64,
        path: PathBuf::new(),
    };
    let path = dir.join(format!("{taken}.snap"));
    // Write to a temporary name first, so a crash never leaves a half-written snapshot behind.
    let tmp = dir.join(format!("{taken}.snap.part"));
    {
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(MAGIC)?;
        let header = serde_json::to_vec(&info).map_err(io::Error::other)?;
        w.write_all(&(header.len() as u32).to_le_bytes())?;
        w.write_all(&header)?;
        let mut z = zstd::stream::Encoder::new(w, 3)?;
        tree.write_to(&mut z)?;
        z.finish()?.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    for old in list(base, root).into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(old.path);
    }
    Ok(path)
}

/// Snapshots of `root`, newest first. Damaged files are skipped.
pub fn list(base: &Path, root: &str) -> Vec<Info> {
    let Ok(rd) = std::fs::read_dir(place_dir(base, root)) else { return Vec::new() };
    let mut out: Vec<Info> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "snap"))
        .filter_map(|p| read_info(&p).ok())
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.taken));
    out
}

fn read_header(r: &mut impl Read, path: &Path) -> io::Result<Info> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Diskie snapshot"));
    }
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 1 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "damaged snapshot header"));
    }
    let mut header = vec![0u8; len];
    r.read_exact(&mut header)?;
    let mut info: Info = serde_json::from_slice(&header).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    info.path = path.to_path_buf();
    Ok(info)
}

/// Reads just the header of a snapshot file.
pub fn read_info(path: &Path) -> io::Result<Info> {
    read_header(&mut BufReader::new(File::open(path)?), path)
}

/// Reads a whole snapshot.
pub fn load(path: &Path) -> io::Result<(Info, Tree)> {
    let mut r = BufReader::new(File::open(path)?);
    let info = read_header(&mut r, path)?;
    let mut z = zstd::stream::Decoder::with_buffer(r)?;
    let tree = Tree::read_from(&mut z)?;
    Ok((info, tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskie_model::tree::flag;
    use diskie_model::{Chunk, NO_KEY};

    #[test]
    fn saves_lists_loads_and_prunes() {
        let base = std::env::temp_dir().join(format!("diskie-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut c = Chunk::default();
        c.push(1, 0, "dir", 0, 5, flag::DIR);
        c.push(NO_KEY, 1, "file.bin", 1234, 5, 0);
        let tree = Tree::build("C:\\Data", 0, vec![c], 2);

        for t in 1..=KEEP as u64 + 2 {
            save(&base, "C:\\Data", &tree, 1000 + t).unwrap();
        }
        save(&base, "C:\\Other", &tree, 5).unwrap();
        let snaps = list(&base, "C:\\Data");
        assert_eq!(snaps.len(), KEEP, "older ones are pruned");
        assert_eq!(snaps[0].taken, 1000 + KEEP as u64 + 2, "newest first");
        assert_eq!(snaps[0].size, 1234);
        assert_eq!(list(&base, "C:\\Other").len(), 1, "places are kept apart");
        let (info, back) = load(&snaps[0].path).unwrap();
        assert_eq!(info.files, 1);
        assert_eq!(back.path(2), tree.path(2));
        assert_eq!(back.size, tree.size);

        // A damaged file is reported, not loaded.
        let bytes = std::fs::read(&snaps[0].path).unwrap();
        std::fs::write(&snaps[0].path, &bytes[..bytes.len() - 20]).unwrap();
        assert!(load(&snaps[0].path).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
