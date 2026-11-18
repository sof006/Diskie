//! `cargo run -p diskie-update --example check_release`: does exactly what an installed copy
//! does against the live GitHub release (manifest, download, checksum, signature), without
//! installing anything. Useful after publishing.
fn main() {
    let current = std::env::args().nth(1).unwrap_or_else(|| "0.0.0".into());
    match diskie_update::check(&current) {
        Ok(Some(u)) => {
            println!("found {} ({} bytes)", u.version, u.asset.size);
            match diskie_update::download(&u, |_, _| {}) {
                Ok(path) => {
                    println!("downloaded and verified: {}", path.display());
                    let _ = std::fs::remove_file(path);
                }
                Err(e) => {
                    eprintln!("FAILED: {e}");
                    std::process::exit(1);
                }
            }
        }
        Ok(None) => println!("{current} is up to date"),
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
