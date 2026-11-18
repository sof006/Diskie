//! `cargo run --release -p diskie-scan --example scan -- C:\ [--no-mft]`: scan and print the biggest folders.
//! `--volumes` lists the drives (mounts on Linux) instead.
use diskie_model::fmt;
use diskie_scan::{Options, Progress, scan};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--volumes") {
        for v in diskie_scan::volumes::list_volumes() {
            println!("{:<28} {:<16} {:<8} {:?} {} free of {}", v.root, v.label, v.filesystem, v.kind, fmt::size(v.free), fmt::size(v.total));
        }
        return;
    }
    let root = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "C:\\".into());
    let opts = Options { mft: !args.iter().any(|a| a == "--no-mft"), threads: None };
    let p = Progress::default();
    let s = scan(std::path::Path::new(&root), opts, &p).expect("scan");
    let t = &s.tree;
    println!(
        "{root}: {} files, {} folders, {}, {} unreadable, {} via {} ({} threads){}",
        fmt::count(t.files[0] as u64),
        fmt::count(t.dirs[0] as u64),
        fmt::size(t.size[0]),
        t.unreadable,
        fmt::duration(s.elapsed.as_secs_f64()),
        s.backend.label(),
        s.threads,
        s.fallback.map(|f| format!(" (MFT not used: {f})")).unwrap_or_default()
    );
    for &k in t.kids(0).iter().take(10) {
        println!("  {:>10}  {}", fmt::size(t.size[k as usize]), t.name(k));
    }
}
