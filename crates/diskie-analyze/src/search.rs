//! Search over a scanned tree with a small query language. Every term must match:
//!
//! | Term | Matches |
//! |---|---|
//! | `report` | names containing "report" (ignoring case) |
//! | `"annual report"` | names containing the phrase |
//! | `*.mkv`, `IMG_????.jpg` | whole names, `*` and `?` as wildcards |
//! | `photos\2019`, `photos/2019` | full paths containing the text |
//! | `-tmp` | names *not* containing "tmp" |
//! | `>1GB`, `<10MB`, `>=500KB`, `size>2G` | sizes (B, KB, MB, GB, TB; 1 KB = 1024 B) |
//! | `older:2y`, `newer:30d` | last modified more or less than this long ago (d, w, m, y) |
//! | `type:video`, `type:archives` | file type, or for a folder the type it holds most of |
//! | `ext:iso` | extension |
//! | `is:file`, `is:folder`, `is:cloud`, `is:link` | what the entry is |

use diskie_model::tree::flag;
use diskie_model::{Kind, Tree};

#[derive(Debug, Default)]
pub struct Query {
    terms: Vec<Term>,
}

#[derive(Debug)]
enum Term {
    Name { text: Vec<u8>, negate: bool },
    Glob(Vec<u8>),
    Path(Vec<u8>),
    Size { op: Op, bytes: u64 },
    Older(u64),
    Newer(u64),
    Type(Kind),
    Ext(Vec<u8>),
    IsFile,
    IsFolder,
    IsCloud,
    IsLink,
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Gt,
    Ge,
    Lt,
    Le,
}

pub struct Results {
    /// Matches, largest first, at most the limit asked for.
    pub nodes: Vec<u32>,
    /// How many entries matched in all.
    pub total: usize,
    /// Bytes in the matches, counting a match inside another match once.
    pub bytes: u64,
}

impl Query {
    pub fn parse(text: &str) -> Result<Query, String> {
        let mut terms = Vec::new();
        for word in split(text) {
            terms.push(term(&word)?);
        }
        Ok(Query { terms })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Searches at and below `under`. `now` is Unix seconds, for ages. Stops early, returning
    /// `None`, once `stop` says so.
    pub fn run(&self, tree: &Tree, under: u32, now: u64, limit: usize, stop: &dyn Fn() -> bool) -> Option<Results> {
        let mut found: Vec<(u64, u32)> = Vec::new();
        let (mut total, mut bytes) = (0usize, 0u64);
        let needs_path = self.terms.iter().any(|t| matches!(t, Term::Path(_)));
        // (node, inside an earlier match)
        let mut stack = vec![(under, false)];
        let mut seen = 0u32;
        while let Some((n, inside)) = stack.pop() {
            seen = seen.wrapping_add(1);
            if seen.is_multiple_of(65_536) && stop() {
                return None;
            }
            let path = if needs_path { tree.path(n) } else { String::new() };
            let hit = n != under && self.matches(tree, n, now, &path);
            if hit {
                total += 1;
                if !inside {
                    bytes += tree.size[n as usize];
                }
                found.push((tree.size[n as usize], n));
                if found.len() > limit * 2 {
                    found.sort_unstable_by_key(|b| std::cmp::Reverse(b.0));
                    found.truncate(limit);
                }
            }
            for &k in tree.kids(n) {
                stack.push((k, inside || hit));
            }
        }
        found.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        found.truncate(limit);
        Some(Results { nodes: found.into_iter().map(|(_, n)| n).collect(), total, bytes })
    }

    fn matches(&self, tree: &Tree, n: u32, now: u64, path: &str) -> bool {
        let name = tree.name(n).as_bytes();
        let i = n as usize;
        let size = if tree.has(n, flag::CLOUD) { tree.online_size(n) } else { tree.size[i] };
        let age = now.saturating_sub(tree.mtime[i] as u64);
        self.terms.iter().all(|t| match t {
            Term::Name { text, negate } => contains(name, text) != *negate,
            Term::Glob(p) => glob(p, name),
            Term::Path(p) => contains(path.as_bytes(), p),
            Term::Size { op, bytes } => match op {
                Op::Gt => size > *bytes,
                Op::Ge => size >= *bytes,
                Op::Lt => size < *bytes,
                Op::Le => size <= *bytes,
            },
            Term::Older(s) => tree.mtime[i] != 0 && age > *s,
            Term::Newer(s) => tree.mtime[i] != 0 && age < *s,
            Term::Type(k) => tree.kind[i] == *k as u8 && (tree.is_dir(n) || size > 0 || tree.has(n, flag::CLOUD)),
            Term::Ext(e) => !tree.is_dir(n) && name.len() > e.len() && name[name.len() - e.len() - 1] == b'.' && eq_ci(&name[name.len() - e.len()..], e),
            Term::IsFile => !tree.is_dir(n),
            Term::IsFolder => tree.is_dir(n),
            Term::IsCloud => tree.has(n, flag::CLOUD | flag::SYNCED),
            Term::IsLink => tree.has(n, flag::REPARSE),
        })
    }
}

/// Words, keeping "quoted phrases" together.
fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn term(w: &str) -> Result<Term, String> {
    let lower = w.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("size")
        && rest.starts_with(['>', '<']) {
            return size_term(rest);
        }
    if w.starts_with(['>', '<']) {
        return size_term(&lower);
    }
    if let Some((key, value)) = lower.split_once(':')
        && !value.is_empty()
        && key.len() > 1
    {
        return match key {
            "older" => Ok(Term::Older(age(value)?)),
            "newer" => Ok(Term::Newer(age(value)?)),
            "ext" => Ok(Term::Ext(value.trim_start_matches('.').as_bytes().to_vec())),
            "type" => Kind::ALL
                .iter()
                .find(|k| k.label().to_ascii_lowercase().starts_with(value) || k.label().to_ascii_lowercase().split(' ').any(|p| p.starts_with(value)))
                .map(|&k| Term::Type(k))
                .ok_or_else(|| format!("Unknown type \"{value}\". Try video, images, audio, archives, disk, apps, documents, code, caches or system.")),
            "is" => match value {
                "file" | "files" => Ok(Term::IsFile),
                "folder" | "folders" | "dir" => Ok(Term::IsFolder),
                "cloud" | "online" => Ok(Term::IsCloud),
                "link" | "links" => Ok(Term::IsLink),
                _ => Err(format!("Unknown \"is:{value}\". Try is:file, is:folder, is:cloud or is:link.")),
            },
            _ => Ok(name_term(w)),
        };
    }
    Ok(name_term(w))
}

fn name_term(w: &str) -> Term {
    if let Some(rest) = w.strip_prefix('-')
        && !rest.is_empty()
    {
        return Term::Name { text: rest.to_ascii_lowercase().into_bytes(), negate: true };
    }
    let lower = w.to_ascii_lowercase().into_bytes();
    if w.contains(['*', '?']) {
        Term::Glob(lower)
    } else if w.contains(['\\', '/']) {
        Term::Path(lower)
    } else {
        Term::Name { text: lower, negate: false }
    }
}

fn size_term(s: &str) -> Result<Term, String> {
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (Op::Ge, r)
    } else if let Some(r) = s.strip_prefix("<=") {
        (Op::Le, r)
    } else if let Some(r) = s.strip_prefix('>') {
        (Op::Gt, r)
    } else if let Some(r) = s.strip_prefix('<') {
        (Op::Lt, r)
    } else {
        return Err(format!("\"{s}\" isn't a size comparison."));
    };
    Ok(Term::Size { op, bytes: parse_size(rest).ok_or_else(|| format!("\"{rest}\" isn't a size. Try 500MB or 2GB."))? })
}

/// `500`, `500kb`, `1.5g`, `2 GiB` (any case) to bytes; units are powers of 1024.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().ok()?;
    let mult: u64 = match unit.trim().trim_end_matches("ib").trim_end_matches('b') {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return None,
    };
    Some((num * mult as f64) as u64)
}

fn age(s: &str) -> Result<u64, String> {
    const DAY: u64 = 86_400;
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("\"{s}\" isn't an age. Try 30d, 6m or 2y."))?;
    let unit = match unit {
        "d" | "day" | "days" => DAY,
        "w" | "wk" | "week" | "weeks" => 7 * DAY,
        "m" | "mo" | "month" | "months" => 30 * DAY,
        "y" | "yr" | "year" | "years" => 365 * DAY,
        _ => return Err(format!("\"{s}\" isn't an age. Try 30d, 6m or 2y.")),
    };
    Ok(n * unit)
}

fn eq_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_ascii_lowercase() == *y)
}

/// Whether `hay` contains `needle` (already lowercase), ignoring ASCII case.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| eq_ci(w, needle))
}

/// `*` and `?` wildcards over the whole name, ignoring ASCII case. `pat` is lowercase.
fn glob(pat: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while n < name.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == name[n].to_ascii_lowercase()) {
            p += 1;
            n += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = Some(p);
            mark = n;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            n = mark;
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::tree;

    fn names(q: &str) -> Vec<String> {
        let t = tree();
        let r = Query::parse(q).unwrap().run(&t, Tree::ROOT, 2_000_000_100, 100, &|| false).unwrap();
        r.nodes.iter().map(|&n| t.name(n).to_string()).collect()
    }

    #[test]
    fn names_globs_and_paths() {
        assert_eq!(names("video"), ["Holiday Video.mkv"]);
        assert_eq!(names("\"holiday video\""), ["Holiday Video.mkv"]);
        assert_eq!(names("*.EXE"), ["setup.exe", "app.exe"]);
        assert_eq!(names("me\\downloads\\"), ["Holiday Video.mkv", "setup.exe"]);
        assert_eq!(names("exe -setup"), ["app.exe"]);
    }

    #[test]
    fn sizes_ages_types() {
        assert_eq!(names(">1gb is:file"), ["pagefile.sys"]);
        assert_eq!(names("size<200 is:file"), ["notes.txt"]);
        assert_eq!(names("type:video is:file"), ["Holiday Video.mkv"]);
        assert_eq!(names("type:video is:folder"), ["Users", "me", "Downloads"], "folders by what they hold most of");
        assert_eq!(names("older:1y ext:exe"), ["setup.exe", "app.exe"]);
        assert_eq!(names("newer:1d is:file"), ["Holiday Video.mkv", "notes.txt"]);
        assert!(Query::parse("type:nonsense").is_err());
        assert!(Query::parse(">huge").is_err());
    }

    #[test]
    fn a_match_inside_a_match_is_counted_once() {
        let t = tree();
        let r = Query::parse("e").unwrap().run(&t, Tree::ROOT, 0, 100, &|| false).unwrap();
        assert!(r.total > 3);
        // Everything matches "e" or is inside a match, except proj and its Cargo.toml.
        assert_eq!(r.bytes, t.size[0] - 1_000);
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("1.5g"), Some(1_610_612_736));
        assert_eq!(parse_size("2 MiB"), Some(2 << 20));
        assert_eq!(parse_size("10"), Some(10));
        assert_eq!(parse_size("ten"), None);
    }

    #[test]
    fn globs() {
        assert!(glob(b"img_????.jpg", b"IMG_2034.JPG"));
        assert!(!glob(b"img_????.jpg", b"IMG_20345.JPG"));
        assert!(glob(b"*", b""));
        assert!(glob(b"a*b*c", b"aXXbYYc"));
    }
}
