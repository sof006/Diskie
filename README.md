# Diskie

A fast disk space analyzer written entirely in Rust, in the same space as TreeSize and WizTree.

> **Status: 0.3.1 preview.** Scanning, the linked tree and treemap, file types, search, largest files, duplicates, clean-up suggestions, changes since the last scan, live updates, age colouring, owners, Recycle Bin sizes, cloud files, export, a command line, deleting to the Recycle Bin or Trash, and self-updates work on Windows and Linux. Still to come: PDF, HTML and XLSX reports, and the built-in SMB client. The design and roadmap are in [PLAN.md](PLAN.md); the framework choice is explained in [spikes/RESULTS.md](spikes/RESULTS.md).

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
| Delete permanently | Shift+Del, or Shift+right-click and *Delete permanently* (always offered where there is no Recycle Bin, such as a network share). A progress bar shows how far it has got, with a Stop button |
| Rescan | F5 |
| Select several | Ctrl+click or Shift+click, in the lists, the tree or the treemap; Ctrl+A in a list. Delete, copy and the menu then act on all of them |
| Search | Type in the box at the top (Ctrl+F). Besides names: `*.iso`, `>1GB`, `<10MB`, `older:2y`, `newer:30d`, `type:video`, `ext:log`, `is:folder`, `is:cloud`, `-word` to exclude, `"exact phrase"`. Every word must match |
| Largest files | The *Largest files* tab: the biggest files below the folder the treemap shows |
| Duplicates | The *Duplicates* tab, then *Find duplicates*. Files are compared by size, then by their first and last 64 KB, then in full; hard links count once and online-only files are never downloaded. *Select extra copies* picks all but one of each, ready to delete |
| Clean-up | The *Clean-up* tab lists what Diskie recognises, such as build output, `node_modules`, caches, crash dumps, old downloads and `Windows.old`, each with what it is and whether it is safe to delete (hover for the details) |
| What changed | Diskie keeps the last 5 scans of each place (in `%LOCALAPPDATA%\Diskie\snapshots`, `~/.local/share/diskie/snapshots` on Linux). The *Changes* tab lists what is new, grew, shrank or is gone since the previous scan; *Colour by: Change* shows it on the treemap, and the panel on the right picks an older scan to compare with |
| Colours | *Colour by* on the right: file type, age (when last changed) or change since an earlier scan |
| Live updates | While a scan is open, changes on disk show up within a few seconds. The *Live* dot in the status bar turns it off (on Linux it needs a watch per folder, so it is off for scans of more than 50,000 folders) |
| Owners | The *Owner* switch next to the tabs adds an owner column |
| Recycle Bin | The sidebar shows how much each drive's Recycle Bin (Trash) holds, with *Empty* |
| Cloud files | OneDrive and other cloud folders on Windows: online-only files are tagged and not counted as using space (the status bar shows how much is online only). Right-click a synced file or folder for *Free up space* or *Always keep on this device* |
| Export | *Export* saves what the current tab lists (the whole folder, for the tree) as CSV, or as JSON if you name the file `.json` |

Settings live in `%APPDATA%\Diskie\settings.json` (`~/.config/Diskie/settings.json` on Linux): `updates` (`"notify"`, `"auto"` or `"off"`), `snapshots` and `live_updates` (`true` or `false`), and on Windows `run_as_admin` and `use_mft`.

### Command line

`Diskie scan <folder>` scans without opening a window and prints JSON: a summary, the folder tree two levels deep and the 20 largest files. `--csv` prints every folder and file as CSV instead; `--depth N` and `--top N` change how much is included, and `--no-mft` walks the folders instead of reading the Master File Table. For example `Diskie scan C:\ --depth 1 > c.json`, or `Diskie scan /home --csv > home.csv`. Run as administrator on Windows for the fast MFT scan; the command line never asks.

## Building

```bash
cargo build --release
```

The executable is `target/release/Diskie.exe` (`target/release/Diskie` on Linux, where `sudo sh packaging/linux/build-deps.sh` first installs the build dependencies on Ubuntu or Debian). `cargo test -p diskie -p diskie-model -p diskie-scan -p diskie-analyze -p diskie-store -p diskie-update` runs the tests. Releases are described in [docs/releasing.md](docs/releasing.md).

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
