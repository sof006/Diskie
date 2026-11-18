//! CSV and JSON output, for the Export button and the command line.

use std::io::{self, Write};

use diskie_model::tree::flag;
use diskie_model::{Kind, Tree};
use serde_json::{Value, json};

/// One row per node: path, type, size in bytes, files, folders, last modified (UTC).
pub fn csv(tree: &Tree, nodes: impl IntoIterator<Item = u32>, w: &mut impl Write) -> io::Result<()> {
    writeln!(w, "Path,Kind,Size,Files,Folders,Modified,Type")?;
    for n in nodes {
        let i = n as usize;
        let what = if tree.is_dir(n) {
            "folder"
        } else if tree.has(n, flag::CLOUD) {
            "online-only file"
        } else if tree.has(n, flag::REPARSE) {
            "link"
        } else {
            "file"
        };
        let size = if tree.has(n, flag::CLOUD) { tree.online_size(n) } else { tree.size[i] };
        writeln!(
            w,
            "{},{what},{size},{},{},{},{}",
            quote(&tree.path(n)),
            tree.files[i],
            tree.dirs[i],
            iso_date(tree.mtime[i]),
            quote(Kind::from_u8(tree.kind[i]).label())
        )?;
    }
    Ok(())
}

/// A flat JSON array of the same fields as [`csv`].
pub fn json_list(tree: &Tree, nodes: impl IntoIterator<Item = u32>, w: &mut impl Write) -> io::Result<()> {
    // Written one entry at a time: a whole drive is millions of them.
    write!(w, "[")?;
    for (i, n) in nodes.into_iter().enumerate() {
        write!(w, "{}
  ", if i == 0 { "" } else { "," })?;
        serde_json::to_writer(&mut *w, &entry(tree, n)).map_err(io::Error::other)?;
    }
    writeln!(w, "
]")
}

/// The tree below `under` as nested JSON, `depth` levels deep (0: just `under`).
pub fn json_tree(tree: &Tree, under: u32, depth: usize) -> Value {
    let mut v = entry(tree, under);
    if depth > 0 && tree.is_dir(under) {
        v["children"] = Value::Array(tree.kids(under).iter().map(|&k| json_tree(tree, k, depth - 1)).collect());
    }
    v
}

fn entry(tree: &Tree, n: u32) -> Value {
    let i = n as usize;
    let mut v = json!({
        "name": tree.name(n),
        "path": tree.path(n),
        "size": if tree.has(n, flag::CLOUD) { tree.online_size(n) } else { tree.size[i] },
        "modified": iso_date(tree.mtime[i]),
        "type": Kind::from_u8(tree.kind[i]).label(),
    });
    if tree.is_dir(n) {
        v["files"] = json!(tree.files[i]);
        v["folders"] = json!(tree.dirs[i]);
    }
    if tree.has(n, flag::CLOUD) {
        v["online_only"] = json!(true);
    }
    if tree.has(n, flag::REPARSE) {
        v["link"] = json!(true);
    }
    v
}

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// `2026-11-18T14:02:00Z`, or an empty string when unknown.
pub fn iso_date(secs: u32) -> String {
    if secs == 0 {
        return String::new();
    }
    let days = secs as i64 / 86_400;
    let rem = secs as i64 % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{find, tree};

    #[test]
    fn dates() {
        assert_eq!(iso_date(0), "");
        assert_eq!(iso_date(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_date(1_794_960_000), "2026-11-18T00:00:00Z");
        assert_eq!(iso_date(1_795_017_600), "2026-11-18T16:00:00Z");
    }

    #[test]
    fn csv_quotes_and_counts() {
        let t = tree();
        let mut out = Vec::new();
        csv(&t, [Tree::ROOT, find(&t, "Holiday Video.mkv")], &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("C:\\,folder,"));
        assert!(lines[2].starts_with("C:\\Users\\me\\Downloads\\Holiday Video.mkv,file,900000000,1,0,"));
        assert!(lines[2].ends_with(",Video"));
    }

    #[test]
    fn nested_json() {
        let t = tree();
        let v = json_tree(&t, Tree::ROOT, 1);
        assert_eq!(v["children"].as_array().unwrap().len(), 4);
        assert!(v["children"][0].get("children").is_none());
        assert_eq!(v["files"], 6);
    }
}
