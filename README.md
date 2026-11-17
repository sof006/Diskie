# Diskie

A fast disk space analyzer written entirely in Rust, in the same space as TreeSize and WizTree.

> **Status: planning.** There is no code yet. The design and roadmap are in [PLAN.md](PLAN.md).

## Goals

- **Fast scanning.** Diskie reads the NTFS Master File Table directly on Windows (with administrator rights, the default), so a full drive takes seconds. Elsewhere it uses a parallel directory walker.
- **Network shares.** It browses and scans SMB shares, including with separate credentials stored in Windows Credential Manager, without mapping a drive.
- **Analysis.** It shows a linked tree and treemap, file types, file age, owners and duplicates, and lets you search and filter.
- **History.** It saves snapshots and compares them over time.
- **Reports.** It exports to PDF, HTML, XLSX, CSV and JSON, from the app or from the command line.
- **Lightweight.** It ships as a single small executable, with low idle memory, and renders on the GPU.
- **Platforms.** Windows first. Linux follows as a tar.gz and an AppImage. On Linux it runs as a normal user, the way Filelight does.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
