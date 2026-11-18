//! Whole-volume scan from the NTFS Master File Table, via `ntfs-reader`.
//!
//! Every in-use file record gives its name, parent folder (by record number), default-stream
//! size and modification time. A file with several hard links is counted once, under its
//! preferred name. Records NTFS keeps for itself (0–23) are not listed by `ntfs-reader`; the
//! big ones ($MFT, $LogFile and so on) are added under the root so the total matches the used
//! space more closely. Anything whose parent chain doesn't reach the root (the `$Extend`
//! metadata folder) is left out.

use std::sync::atomic::Ordering::Relaxed;

use diskie_model::tree::flag;
use diskie_model::{Chunk, NO_KEY, Tree};
use ntfs_reader::{Mft, Volume};

use crate::{Phase, Progress};

const ROOT_RECORD: u64 = 5;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// Metadata files worth showing: $MFT, $MFTMirr, $LogFile, $Bitmap.
const METADATA: [(u64, &str); 4] = [(0, "$MFT"), (1, "$MFTMirr"), (2, "$LogFile"), (6, "$Bitmap")];

pub fn scan(letter: char, progress: &Progress) -> Result<Tree, String> {
    let device = format!(r"\\.\{letter}:");
    progress.set_current(&format!("{letter}:\\ (Master File Table)"));
    let volume = Volume::new(&device).map_err(|e| format!("{e}"))?;
    let mft = Mft::new(volume).map_err(|e| format!("{e}"))?;
    if progress.is_cancelled() {
        return Err("cancelled".into());
    }

    let records = mft.record_count();
    let mut chunk = Chunk::with_capacity(records as usize);
    let (mut files, mut dirs, mut bytes) = (0u64, 0u64, 0u64);
    for (i, file) in mft.files().enumerate() {
        if i % 65_536 == 0 {
            progress.files.store(files, Relaxed);
            progress.dirs.store(dirs, Relaxed);
            progress.bytes.store(bytes, Relaxed);
            if progress.is_cancelled() {
                return Err("cancelled".into());
            }
        }
        let Some(name) = file.best_name() else { continue };
        let parent = name.parent_number();
        let name = name.to_string();
        let info = file.standard_information();
        let mtime = info.as_ref().map(|s| s.modified().unix_timestamp().clamp(0, u32::MAX as i64) as u32).unwrap_or(0);
        let reparse = info.as_ref().is_some_and(|s| s.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0);
        if file.is_directory() {
            let f = flag::DIR | if reparse { flag::REPARSE } else { 0 };
            chunk.push(file.number(), parent, &name, 0, mtime, f);
            dirs += 1;
        } else {
            let size = if reparse { 0 } else { file.data_streams().find(|s| s.name.is_none()).map(|s| s.size).unwrap_or(0) };
            chunk.push(NO_KEY, parent, &name, size, mtime, if reparse { flag::REPARSE } else { 0 });
            files += 1;
            bytes += size;
        }
    }
    for (number, name) in METADATA {
        if let Some(rec) = mft.record(number) {
            let size = rec.data_streams().find(|s| s.name.is_none()).map(|s| s.size).unwrap_or(0);
            chunk.push(NO_KEY, ROOT_RECORD, name, size, 0, flag::SYNTHETIC);
            bytes += size;
        }
    }
    progress.files.store(files, Relaxed);
    progress.dirs.store(dirs, Relaxed);
    progress.bytes.store(bytes, Relaxed);
    drop(mft);

    progress.set_phase(Phase::Building);
    Ok(Tree::build(&format!("{letter}:\\"), ROOT_RECORD, vec![chunk], records.max(ROOT_RECORD + 1)))
}
