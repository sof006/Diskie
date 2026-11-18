//! Command-line use, for scripts and scheduled reports:
//!
//! ```text
//! Diskie scan <folder> [--json | --csv] [--depth N] [--top N] [--no-mft]
//! ```

use std::io::Write;
use std::path::PathBuf;

use diskie_analyze::{export, top};
use diskie_model::Tree;

pub const USAGE: &str = "\
Diskie: a fast disk space analyzer

Usage:
  Diskie [folder]                 Open the window (and scan the folder)
  Diskie scan <folder> [options]  Scan and print the result, without a window

Options for scan:
  --json        A summary, the folder tree and the largest files as JSON (the default)
  --csv         One line per folder and file, as CSV
  --depth N     How many levels of folders to include (JSON: 2, CSV: all)
  --top N       How many of the largest files to list in JSON (default 20)
  --no-mft      Don't read the NTFS Master File Table, walk the folders instead
  --help        Show this text

Examples:
  Diskie scan C:\\ --depth 1
  Diskie scan /home --csv > home.csv
";

/// A folder given on the command line. Windows reads `"C:\"` as `C:"` (the backslash escapes
/// the closing quote), so a trailing quote is taken to be that backslash.
pub fn path_arg(a: &str) -> PathBuf {
    match a.strip_suffix('"') {
        Some(rest) if cfg!(windows) => PathBuf::from(format!("{rest}\\")),
        Some(rest) => PathBuf::from(rest),
        None => PathBuf::from(a),
    }
}

/// Whether these arguments ask for the command line rather than the window.
pub fn wanted(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("scan" | "--help" | "-h" | "/?" | "--version" | "-V"))
}

/// Runs the command line and returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    attach_console();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("Diskie {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        Some("scan") => {}
        _ => {
            print!("{USAGE}");
            return 0;
        }
    }
    let mut folder = None;
    let (mut csv, mut depth, mut top_n, mut mft) = (false, None, 20usize, true);
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut number = |name: &str| -> Result<usize, String> {
            it.next().and_then(|v| v.parse().ok()).ok_or_else(|| format!("{name} needs a number"))
        };
        let r = match a.as_str() {
            "--json" => {
                csv = false;
                Ok(())
            }
            "--csv" => {
                csv = true;
                Ok(())
            }
            "--depth" => number("--depth").map(|n| depth = Some(n)),
            "--top" => number("--top").map(|n| top_n = n),
            "--no-mft" => {
                mft = false;
                Ok(())
            }
            "--no-elevate" => Ok(()),
            "--help" | "-h" => {
                print!("{USAGE}");
                return 0;
            }
            s if s.starts_with("--") => Err(format!("unknown option {s}")),
            s => {
                folder = Some(path_arg(s));
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("Diskie: {e}. See Diskie --help.");
            return 2;
        }
    }
    let Some(folder) = folder else {
        eprintln!("Diskie: scan needs a folder, drive or share. See Diskie --help.");
        return 2;
    };

    let progress = diskie_scan::Progress::default();
    let scan = match diskie_scan::scan(&folder, diskie_scan::Options { mft, threads: None }, &progress) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Diskie: couldn't scan {}: {e}", folder.display());
            return 1;
        }
    };
    let tree = &scan.tree;
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    let result = if csv {
        let max = depth.unwrap_or(usize::MAX);
        let nodes = tree.subtree(Tree::ROOT).filter(|&n| (tree.depth[n as usize] as usize) <= max);
        export::csv(tree, nodes, &mut out)
    } else {
        let largest: Vec<serde_json::Value> = top::largest_files(tree, Tree::ROOT, top_n)
            .into_iter()
            .map(|n| serde_json::json!({ "path": tree.path(n), "size": tree.size[n as usize], "modified": export::iso_date(tree.mtime[n as usize]) }))
            .collect();
        let doc = serde_json::json!({
            "root": tree.name(Tree::ROOT),
            "scanned_at": export::iso_date(crate::settings::now() as u32),
            "seconds": (scan.elapsed.as_secs_f64() * 1000.0).round() / 1000.0,
            "method": scan.backend.label(),
            "mft_not_used": scan.fallback,
            "size": tree.size[0],
            "files": tree.files[0],
            "folders": tree.dirs[0],
            "unreadable_folders": tree.unreadable,
            "free_space": diskie_scan::volumes::space(&folder).map(|(total, free)| serde_json::json!({ "total": total, "free": free })),
            "tree": export::json_tree(tree, Tree::ROOT, depth.unwrap_or(2)),
            "largest_files": largest,
        });
        serde_json::to_writer_pretty(&mut out, &doc).map_err(std::io::Error::other).and_then(|_| writeln!(out))
    };
    match result.and_then(|_| out.flush()) {
        Ok(()) => 0,
        // The reader went away (`| head`): not an error worth reporting.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            eprintln!("Diskie: couldn't write the output: {e}");
            1
        }
    }
}

/// A release build on Windows is a GUI program with no console of its own. When started from
/// one, write to that console; when the output is redirected, it already goes to the file.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn recognises_command_line_use() {
        let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(wanted(&a(&["scan", "C:\\"])));
        assert!(wanted(&a(&["--help"])));
        assert!(!wanted(&a(&["C:\\"])));
        assert!(!wanted(&a(&["--no-elevate", "D:\\"])));
        assert!(!wanted(&[]));
    }

    #[::core::prelude::v1::test]
    fn a_quoted_drive_root_survives() {
        assert_eq!(path_arg(r"D:\Games"), PathBuf::from(r"D:\Games"));
        if cfg!(windows) {
            assert_eq!(path_arg(r#"C:""#), PathBuf::from(r"C:\"));
            assert_eq!(path_arg(r#"D:\My Folder""#), PathBuf::from(r"D:\My Folder\"));
        }
    }
}
