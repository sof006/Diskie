# Diskie

A fast disk space analyzer written entirely in Rust, in the same space as TreeSize and WizTree.

> **Status: 0.1.0 preview.** Scanning, the linked tree and treemap, file types, Recycle Bin clean-up and self-updates work on Windows. Reports, snapshots, search and Linux builds are still to come. The design and roadmap are in [PLAN.md](PLAN.md); the framework choice is explained in [spikes/RESULTS.md](spikes/RESULTS.md).

## Download

Get `Diskie-<version>-windows-x64.exe` from [Releases](https://github.com/sof006/Diskie/releases) and run it. There is nothing to install. Diskie asks for administrator rights at startup for the fastest scans; decline and it still works, using the slower folder walk. After that it keeps itself up to date, and every update is checked against the project's signing key before it is installed.

## Using it

| | |
|---|---|
| Scan a drive | Click it in the sidebar |
| Scan a folder or network share | *Scan folder or share…* (Ctrl+O); type a `\\server\share` path for a share |
| Move around | Arrow keys; Right/Left expand and collapse |
| Zoom the treemap | Double-click or Enter; Backspace goes back up |
| Act on a file or folder | Right-click: *Show in Explorer*, *Copy path*, *Move to Recycle Bin* (or Del) |
| Rescan | F5 |

Settings live in `%APPDATA%\Diskie\settings.json`: `updates` (`"notify"`, `"auto"` or `"off"`), `run_as_admin` and `use_mft`.

## Building

```bash
cargo build --release
```

The executable is `target/release/Diskie.exe`. `cargo test -p diskie-model -p diskie-scan -p diskie-update` runs the tests. Releases are described in [docs/releasing.md](docs/releasing.md).

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
