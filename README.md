# Diskie

A fast disk space analyzer written entirely in Rust, in the same space as TreeSize and WizTree.

> **Status: 0.2.1 preview.** Scanning, the linked tree and treemap, file types, clean-up to the Recycle Bin or Trash, and self-updates work on Windows and Linux. Reports, snapshots and search are still to come. The design and roadmap are in [PLAN.md](PLAN.md); the framework choice is explained in [spikes/RESULTS.md](spikes/RESULTS.md).

## Download

Get `Diskie-<version>-windows-x64.exe` from [Releases](https://github.com/sof006/Diskie/releases) and run it. There is nothing to install. Diskie asks for administrator rights at startup for the fastest scans; decline and it still works, using the slower folder walk. After that it keeps itself up to date, and every update is checked against the project's signing key before it is installed.

On Linux (x86-64, glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 and later), either:

- download `Diskie-<version>-x86_64.AppImage`, make it executable (`chmod +x`) and run it; or
- unpack `Diskie-<version>-linux-x64.tar.gz` and run `./install.sh`, which puts `diskie` in `~/.local/bin` and adds it to your applications menu (`./install.sh --uninstall` removes it).

Both keep themselves up to date the same way. Diskie runs as your normal user on Linux: folders you can't read are marked, and a scan stays on the file system it started on (other mounts, such as `/proc` or a USB stick, are shown but not entered; double-click one to scan it).

## Using it

| | |
|---|---|
| Scan a drive | Click it in the sidebar |
| Scan a folder or network share | *Scan folder or share…* (Ctrl+O); type a `\\server\share` path for a share |
| Move around | Arrow keys; Right/Left expand and collapse |
| Zoom the treemap | Double-click or Enter; Backspace goes back up |
| Act on a file or folder | Right-click: *Show in Explorer* (*file manager* on Linux), *Copy path*, *Move to Recycle Bin* (*Trash*) or Del |
| Delete permanently | Shift+Del, or Shift+right-click and *Delete permanently* (always offered where there is no Recycle Bin, such as a network share) |
| Rescan | F5 |

Settings live in `%APPDATA%\Diskie\settings.json` (`~/.config/Diskie/settings.json` on Linux): `updates` (`"notify"`, `"auto"` or `"off"`), and on Windows `run_as_admin` and `use_mft`.

## Building

```bash
cargo build --release
```

The executable is `target/release/Diskie.exe` (`target/release/Diskie` on Linux, where `sudo sh packaging/linux/build-deps.sh` first installs the build dependencies on Ubuntu or Debian). `cargo test -p diskie-model -p diskie-scan -p diskie-update` runs the tests. Releases are described in [docs/releasing.md](docs/releasing.md).

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
