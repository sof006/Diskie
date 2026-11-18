# Diskie: plan

A disk space analyzer written entirely in Rust, aimed at the same space as TreeSize and WizTree.
The goals are WizTree-class scan speed, TreeSize-class features (SMB shares, reports, snapshots, owners, duplicates), a small footprint, and an interface that looks like a well-made tool, not a template.

> Status: **0.2.1 preview released** (2026-11-18): scanning (MFT and parallel walk), the linked tree and treemap, file types, Recycle Bin, themes and the signed self-updater. Windows is the main platform. **Linux (phase 7) shipped in 0.2.0**: mounts and free space, a one-file-system walk that counts hard links once, a self-drawn window frame where the desktop has none, Trash, and the tar.gz and AppImage with self-update, tested under WSLg (Wayland). Still open for Linux: testing on X11 and real Ubuntu, Fedora and Arch desktops, and the built-in SMB client. Open source. Decisions are recorded in §8.

---

## 1. Targets

| Metric | Target | Notes |
|---|---|---|
| Local NTFS scan, 1M files, admin | < 2 s | Read the MFT directly, the way WizTree and Everything do |
| Local scan, 1M files, no admin | < 10 s | Parallel `NtQueryDirectoryFile` with large buffers |
| SMB share, 1M files, LAN | Network-bound; 3–5× faster than Explorer-style enumeration | Many directories enumerated concurrently, no per-file stat calls |
| Memory per file node | ≤ 64 bytes average, including names | 10M files ≈ 640 MB worst case |
| Idle RAM, no scan loaded | < 40 MB | |
| Cold start to first frame | < 200 ms | |
| Release binary | < 20 MB, single `.exe`, no installer needed | A portable build, plus an optional MSI |
| UI frame time with 10M nodes loaded | < 8 ms (120 Hz-capable) | Virtualized lists, treemap layout off the UI thread |

---

## 2. GUI framework

The UI has to do four hard things: a virtualized tree-table with millions of rows, a treemap with up to roughly 200k rectangles drawn every frame, crisp text with tabular numerals, and a fast frame rate. Here is how the realistic options compare in late 2026:

| | **GPUI + GPUI Kit** (Zed) | **iced 0.14** | **egui** | **Slint** | **Freya 0.4** |
|---|---|---|---|---|---|
| Rendering | GPU (DirectX 11 on Windows, Metal, Vulkan) | wgpu / tiny-skia | wgpu / glow | femtovg / Skia / software | Skia |
| Large virtual table | Built in (GPUI Kit `Table`, sized for hundreds of thousands of rows) | Build it yourself | `egui_extras` table, good | `ListView`, workable | Build it yourself |
| Custom drawing (treemap) | `canvas()` + `paint_quad`, very fast | `Canvas` widget, good | Painter API, good | Limited (no immediate canvas) | Skia canvas, good |
| Visual ceiling | Highest (Zed-level polish) | High (Halloy, COSMIC) | Medium; Rerun shows it can be pushed | Medium–high | High |
| API stability | Pre-1.0; crates.io release lags Zed's main branch | Pre-1.0 but steady | Stable-ish | 1.x, stable | Just rewrote for 0.4 |
| License | Apache-2.0 | MIT | MIT/Apache | GPLv3, or royalty-free **with mandatory attribution**, or paid | MIT |
| Windows maturity | Zed has shipped a stable Windows build since Oct 2025 | Good | Good | Good | OK |

### Recommendation: GPUI + GPUI Kit, with iced as the fallback

- GPUI is the only Rust stack that ships a world-class desktop app (Zed) on all three platforms. Its text rendering, hairline borders, and input latency are noticeably better than the alternatives, which is most of what "visually appealing and fast" comes down to.
- GPUI Kit (formerly `gpui-component`, from Longbridge) already provides the hardest widgets: a virtualized table with resizable, sortable columns, a dock with resizable panels, tabs, a command palette, inputs, menus, and charts. Longbridge Pro, a trading desktop app, uses it in production, which is the same "dense data at scale" problem we have.
- The risk is API churn. Mitigations:
  1. Pin an exact GPUI and GPUI Kit version (GPUI Kit's own pinning crate does this) and upgrade on purpose, not automatically.
  2. Keep **all** logic in UI-agnostic crates (§4). The UI crate only renders state and sends commands, so moving to iced would mean rewriting one crate, not the app.
- **Week-1 spike: done, GPUI chosen.** The same prototype (2M-row virtual tree, 144k-rectangle treemap) was built in both frameworks; see [spikes/RESULTS.md](spikes/RESULTS.md).
  - Both stay far under the frame budget once the treemap is a cached bitmap: GPUI about 2.8 ms of CPU per frame, iced about 1 ms.
  - GPUI reaches its first frame in about 0.25 s against iced's 1.1–2.8 s, and uses 40 MB of baseline memory against 240 MB.
  - Frame rates and text quality on a connected display are still to be checked.

**Ruled out:** Tauri, Dioxus-webview and Electron-style stacks (not "completely Rust" in the UI, and heavier). Slint (the attribution license and the missing immediate-mode canvas both hurt the treemap). GTK4-rs (poor Windows experience).

---

## 3. Reference apps

These are well-made Rust apps worth studying, and what each one teaches.

| App | Stack | What to take from it |
|---|---|---|
| **[Zed](https://zed.dev)** | GPUI | The overall feel: dense but calm layouts, 1px dividers instead of boxes, an excellent command palette, keyboard-first, everything instant. Theme files as data. |
| **Longbridge Pro** | GPUI + GPUI Kit | Huge sortable tables that stay smooth; how financial UIs use color *only* to carry meaning. |
| **[nohrs](https://github.com/zed-industries/awesome-gpui)** (GPUI file explorer, macOS) | GPUI | The closest live example of a GPUI file browser: breadcrumb, list, preview patterns. |
| **Zedis** / **zqlz** (Redis GUI / database IDE) | GPUI | A tree sidebar plus a data grid plus a detail panel: the same three-pane shape Diskie needs. |
| **[Halloy](https://github.com/squidowl/halloy)** (IRC) | iced | Restraint. A plain-TOML theme system with community themes. Proof iced can look refined. |
| **COSMIC Files** | iced (libcosmic) | File-manager conventions (tabs, breadcrumbs, network locations) done in Rust. |
| **[Sniffnet](https://github.com/GyulyVGC/sniffnet)** (network monitor) | iced | Live charts and stats panels, themeable, cross-platform packaging done well. |
| **[Rerun viewer](https://rerun.io)** | egui (`re_ui`) | A disciplined design-token system on top of a "tool" framework. Good model for our token file. |
| **Krokiet** (Czkawka's new UI) | Slint | The closest functional cousin (duplicate and large-file finding). Study its results UX, and its limits. |
| **Mission Center** | GTK4-rs | GPU-drawn graphs with low CPU use; a clear information hierarchy for system data. |

Algorithm and UX references in the same domain (concepts only, no code copying):
- **WizTree**: MFT scanning, and the "file types" side panel coloring the treemap.
- **SpaceSniffer**: nested treemap with folder header strips, so you can read the hierarchy at a glance.
- **WinDirStat**: cushion-shaded treemap (Van Wijk & Van de Wetering). Its code is GPL, so we implement from the paper.
- **[pdu](https://github.com/KSXGitHub/parallel-disk-usage)**, **dua-cli**, **dust**: parallel directory walking strategies and their published benchmarks.

---

## 4. Architecture

A Cargo workspace. Everything except `diskie-ui` is headless and testable.

```
diskie/
├─ crates/
│  ├─ diskie-model     # Arena tree, string interning, aggregation, sort, filter, diff
│  ├─ diskie-scan      # Scanner trait + backends (MFT, Win32, POSIX, SMB)
│  ├─ diskie-net       # Share discovery, credentials, connection management
│  ├─ diskie-analyze   # File types, age histogram, owners, duplicates, top-N
│  ├─ diskie-store     # Snapshot save/load (rkyv + zstd), history, diffing
│  ├─ diskie-report    # CSV / JSON / XLSX / HTML / PDF export
│  ├─ diskie-ui        # GPUI app; the only crate that knows about the UI framework
│  └─ diskie-cli       # Headless scans, exports, scheduled jobs, CI-friendly
└─ xtask/              # Build, packaging, benchmarks, fixture generation
```

### 4.1 Tree model (`diskie-model`)

The model is the memory and speed foundation, so it gets the most care.

- **Struct-of-arrays arena**, with `u32` node indices instead of `Box`/`Rc`:
  - `parent: Vec<u32>`, `first_child: Vec<u32>`, `next_sibling: Vec<u32>`
  - `size: Vec<u64>`, `allocated: Vec<u64>` (size on disk), `mtime: Vec<u32>` (minutes since epoch)
  - `name: Vec<NameRef>` into a single `Vec<u8>` UTF-8 name arena (~6 bytes per node plus the name bytes)
  - `flags: Vec<u16>`: dir, hardlink, reparse point, compressed, sparse, cloud placeholder, offline, access denied, and so on
  - Optional columns are allocated only when that analysis is enabled: owner SID index, file-type index, hash.
- Aggregation is a single reverse-topological pass, so totals are O(n) after the scan.
- Children sorted by size are computed lazily per expanded folder and cached, never for the whole tree.
- Hardlinks are counted once (tracked by file ID); the UI shows that they are hardlinks.
- Live updates while scanning: the scanner sends batches over a channel, the model applies them, and the UI re-reads at most once per frame. Results show up within ~50 ms of starting.

### 4.2 Scan backends (`diskie-scan`)

All backends implement one trait, `Scanner::scan(root, opts, sink)`, and are chosen automatically:

| Backend | When | How |
|---|---|---|
| **MFT reader** | Local NTFS volume, elevated | Open `\\.\C:`, read `$MFT` in large sequential chunks, parse FILE records in parallel (rayon), rebuild the tree from parent references. Start from `ntfs-reader` / `usn-journal-rs`, or a custom parser if they are too slow or allocate too much. This is where WizTree's speed comes from. |
| **USN journal refresh** | After an MFT scan | Apply changes from the change journal for near-instant rescans of the same volume. |
| **Win32 fast enumerator** | Not elevated, ReFS/FAT/exFAT, **and SMB via UNC paths** | `NtQueryDirectoryFile` with `FileIdExtdDirectoryInformation` and 64–256 KB buffers. Size, times and attributes come back with the directory listing, so there are no per-file `stat` calls. A work-stealing pool, with concurrency tuned per volume: high (32–64) for SMB, where latency rather than throughput is the limit, and **1–2 for hard disks**, detected through the device's seek penalty. In the spike, a fully parallel walk of a cold 1.7M-entry hard disk did not finish in 5 minutes; with a warm cache it took 2.3 s. `\\?\` long-path prefix everywhere. |
| **POSIX enumerator** | Linux (phase 7) | `openat` + `getdents64` + `statx`, the same work-stealing pool. Runs as the normal user, the way Filelight does (see §4.8). |
| **Native SMB client** | Browsing a share with different credentials *without* mapping a drive; Linux/macOS; servers where Win32 enumeration is slow | Pure-Rust SMB2/3 via the [`smb2`](https://docs.rs/crate/smb2) crate (pipelined I/O, signing and encryption, built for a file-manager use case) or [`smb`](https://docs.rs/crate/smb) (smb-rs: NTLM and Kerberos via `sspi`, QUIC, multichannel). Benchmark both in phase 3. |

Correctness rules that every backend follows:
- Never follow junctions or symlinks by default, which prevents loops and double counting. Show them as "link → target".
- **Never hydrate cloud placeholders** (OneDrive, Dropbox, iCloud). Honor `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / `RECALL_ON_OPEN`, and report logical size versus local size separately.
- Logical size and allocated size are both stored. Compressed, sparse and slack space are visible as columns.
- Access-denied folders are recorded as nodes with a flag, and listed in an "Unreadable" panel with an "Elevate and rescan" action.
- Scans can be cancelled at any time and pause cleanly. Progress reports both throughput (files/s) and an estimate (MFT record count when known).

### 4.3 Network and SMB (`diskie-net`)

Matching what TreeSize does with network drives:
- **Discovery:** mapped drives (`WNetOpenEnum`), shares on a given server (`NetShareEnum`), DFS namespaces (`NetDfsEnum`), and recently used servers. A "Network" section in the sidebar works like Explorer's but loads faster.
- **Connect as…:** alternate credentials per server, stored in Windows Credential Manager (via the `keyring` crate). Never stored in our own files.
- **Multiple roots in one scan:** for example `\\fs01\home$` and `\\fs02\projects` scanned in parallel and shown side by side or merged.
- **Resumable scans** for very large shares, with checkpointing to `diskie-store`.
- **Server-friendly throttling:** a configurable concurrency cap per server so a scan doesn't hammer a production NAS.

### 4.4 Analysis (`diskie-analyze`)

Every analysis runs on the finished (or still-filling) model in the background and never blocks the UI.
- **File types:** grouped by extension, with editable categories (Video, Archives, Disk images, Build output, Caches…). These drive the treemap colors.
- **Top N:** largest files, largest folders, and fastest-growing folders (needs history).
- **Age:** a histogram by last modified and last accessed time, with "files not touched in N years" as a one-click filter.
- **Owners (NTFS):** resolve owner SIDs to account names, cached per scan. Useful for file-server cleanup.
- **Duplicates:** group by size, then hash the first and last 64 KB with xxh3, then a full BLAKE3 only on remaining candidates. Hardlink-aware.
- **Search and filters:** name glob or regex, size, date, attributes, and owner. Results appear in a flat virtual list and can be exported.

### 4.5 Snapshots and history (`diskie-store`)

- Save any scan as a compact snapshot: an `rkyv` zero-copy archive compressed with zstd. Rough estimate: 10M nodes ≈ 150–250 MB on disk, and it opens instantly via mmap.
- **Compare two snapshots:** per-folder growth or shrinkage, with new and deleted files highlighted in the tree and treemap.
- Scheduled scans via the CLI plus Windows Task Scheduler, building up a history per root.

### 4.6 Reports (`diskie-report`)

CSV and JSON (streamed, so huge outputs are fine), XLSX (`rust_xlsxwriter`), a self-contained HTML report with an inline SVG treemap, and **PDF (in v1)**. The PDF is built with `krilla`, the PDF writer underneath Typst, which is far lighter than embedding Typst itself. It has a cover summary, a vector treemap, top folders and files, file types, age, and owners, plus growth since the last snapshot when one exists. It is generated from the GUI or with `diskie report --pdf` for scheduled runs.

### 4.7 Actions

Reveal in Explorer, open, copy path, **Delete to Recycle Bin** (default) or permanent delete (with an explicit confirmation listing the count and total size), move to another folder, and "Compress with NTFS compression". After any action the model updates in place, with no rescan. An optional Explorer context-menu entry, "Analyze with Diskie".

### 4.8 Privileges per platform

**Windows: runs as administrator by default.**
- At launch, Diskie relaunches itself through the UAC prompt (`ShellExecute` with `runas`), so MFT scanning is available, as with WizTree. *Changed 2026-11-18:* GPUI embeds its own application manifest, so a `requireAdministrator` manifest isn't possible. Relaunching also behaves better: if the prompt is declined, Diskie carries on as a standard user instead of refusing to start.
- A `--no-elevate` flag and the `run_as_admin` setting allow a standard-user run, which falls back to the directory walk.
- Two Windows behaviours to handle, because elevated processes run in a separate logon session:
  1. **Mapped drive letters made by the normal user are invisible to an elevated process.** Diskie reads the user's mappings (`HKCU\Network` and `WNetGetConnection` from the unelevated token) and offers them in the sidebar as their UNC paths. UNC paths and shares added in Diskie itself always work.
  2. **Drag-and-drop from a non-elevated Explorer into an elevated window is blocked** by Windows (UIPI). Instead, the Explorer context-menu entry passes the path on the command line.

**Linux: runs as the normal user, like Filelight.**
- No root and no setuid. Diskie scans everything the user can read, and each unreadable folder is marked and counted in the "Unreadable" panel rather than failing the scan.
- Its size is shown as unknown, not zero.
- Mount points are shown, and by default a scan does not cross into other filesystems (`/proc`, `/sys`, network mounts), with a toggle to allow it.
- SMB shares come from existing mounts (`cifs`, or GVFS under `/run/user/<uid>/gvfs`) or through Diskie's built-in SMB client, so no root is needed to mount anything.
- Optional: a "Rescan as administrator" action that re-runs only the scanner through `pkexec diskie scan --json` and streams the results back, so the UI itself never runs as root. Planned as a later addition, not v1.

### 4.9 Updates from GitHub Releases (`diskie-update`)

Diskie keeps itself current from the project's GitHub Releases, so nobody has to revisit the releases page.

**How a release is published (GitHub Actions, on every version tag):**
1. CI builds the Windows `.exe` (portable) and `.msi`, plus the Linux `.tar.gz` and `.AppImage`.
2. It signs every file with a **minisign (Ed25519) key** held only in the repository's Actions secrets, and adds a small `latest.json` manifest to the release. The manifest gives the version, the release-notes summary, and the download URL, SHA-256 and signature for each platform's file. Windows files are also Authenticode-signed once a certificate is in place, so SmartScreen doesn't warn.

**How the app checks:**
- It fetches `https://github.com/sof006/Diskie/releases/latest/download/latest.json`. That is a plain file download, not the GitHub API, so there is no API rate limit and no token. Pre-releases have their own manifest, for anyone who opts into a "Preview" channel.
- **When:** at most once a day, in the background a few seconds after launch, and on demand from *Help › Check for updates*. The CLI has `diskie update --check` and `diskie update`.
- **Privacy:** that request goes only to github.com and sends nothing but a normal HTTP request. There is no telemetry, and the settings page says so.

**How it installs:**
1. It downloads the file, then **verifies the SHA-256 and the minisign signature against the public key compiled into Diskie** before touching anything. This matters doubly because Diskie runs as administrator on Windows: an unsigned or tampered file is rejected, and HTTPS alone is not trusted to be enough.
2. **Windows, portable `.exe`:** a running executable can't be overwritten, but it can be renamed. Diskie renames itself to `Diskie.old.exe`, puts the new file in its place (the `self-replace` crate), and offers *Restart now*.
3. **Windows, MSI install:** it downloads the new `.msi` and hands it to `msiexec`, so Programs and Features stays accurate.
4. **Linux AppImage:** it replaces the file `$APPIMAGE` points to (and renames it to the new version, as on Windows). zsync update information for AppImageUpdate is not embedded yet.
5. **Linux tar.gz:** `install.sh` puts the binary in `~/.local/bin`, and Diskie replaces it from the release's bare `Diskie-<version>-linux-x64` download. A binary the user can't write to, or one under `/usr`, `/opt`, `/nix` or `/snap` (a package), is left to whatever installed it.
6. Builds packaged by a distribution, if any appear, turn self-update off and leave updates to the package manager.

**Safety:** if the new version fails to start cleanly twice, Diskie restores `Diskie.old.exe` (or the previous AppImage) on the next launch and says why. The old copy is deleted after the first successful run of the new one.

**What the user sees:**
- **A notice, not a dialog:** a quiet line in the status bar, "Diskie 0.4.0 is available · What's new · Update". Nothing pops up over a running scan, and nothing restarts by itself mid-scan.
- **One setting:** *Install automatically* (applies on the next launch), *Notify only* (the default), or *Off*.
- **Release notes:** taken from the GitHub release text and shown in-app.

**Crates:** `ureq` (small HTTP client, rustls), `minisign-verify`, `sha2`, `self-replace`, `semver`.

---

## 5. Interface

### 5.1 Layout

```
┌────────────────────────────────────────────────────────────────────────────┐
│ ◂ ▸  C:\ › Users › dev › AppData                     ⌘K Search   ⟳  ⋯     │  title bar = breadcrumb + actions
├──────────────┬───────────────────────────────────────────┬─────────────────┤
│ LOCATIONS    │ Name                 Size    %  Files  Mod │ FILE TYPES      │
│  C:  412 GB  │ ▾ AppData          88.2 GB ███▌ 412k  2d   │ ■ Video  31.0 GB│
│  D:  1.8 TB  │   ▸ Local          71.9 GB ██▉  380k  2d   │ ■ Cache  18.4 GB│
│ NETWORK      │   ▸ Roaming        16.1 GB ▋     31k  5h   │ ■ Images  9.9 GB│
│  \\fs01      │   ▸ LocalLow        0.2 GB        1k  3w   │ ■ Code    4.1 GB│
│   home$      │                                           │   …             │
│   projects   │                                           │                 │
│ SNAPSHOTS    ├───────────────────────────────────────────┴─────────────────┤
│  C: Nov 01   │                                                             │
│              │                  TREEMAP (resizable split)                   │
│              │                                                             │
├──────────────┴─────────────────────────────────────────────────────────────┤
│ Scanned 1,284,113 files · 412.4 GB · 1.6 s (MFT)        Free 88 GB of 1 TB │
└────────────────────────────────────────────────────────────────────────────┘
```

- The tree and treemap stay in sync: hovering a row highlights its rectangle and the reverse. Clicking a rectangle selects and reveals the row. Double-clicking zooms in.
- The treemap/tree split is resizable, and either pane can be collapsed. The layout is saved.
- Tabs at the top let you keep several scans open, such as C: and a network share.
- Everything is reachable from the command palette (Ctrl+K) and the keyboard: arrows to navigate, Enter to zoom, Backspace to go up, Del to send to the Recycle Bin.

### 5.2 Visual direction: avoiding the generic look

The aim is a precise instrument, closer to a good IDE or an audio tool than to a SaaS dashboard.

**Not doing:**
- Purple/blue gradients, glassmorphism, neon glows, or "aurora" backgrounds.
- Cards with drop shadows floating on a grey canvas, and rounded-everything (radius ≥ 12 px on every surface).
- Emoji as icons; big hero numbers with tiny labels; the same accent color on every element.
- Default Inter at one weight everywhere; centered empty-state illustrations with "Let's get started!" copy.
- Overlays where a plain divider would do; animation that exists only for its own sake.

**Doing:**
- **Surfaces separated by 1 px hairlines and slight tone shifts**, not shadows. Radius 4–6 px on controls only; panels are square.
- **Typography:** Segoe UI Variable on Windows (native, crisp with ClearType-equivalent rendering) and SF on macOS, with **tabular numerals** in every size column so digits line up. A monospace face (Cascadia Mono / JetBrains Mono) only for paths and hashes. Three sizes and two weights in total.
- **Color only carries meaning.** A near-neutral UI (warm-grey dark theme and paper-white light theme), with color reserved for the treemap's file-type categories and the size bars. The category palette is generated in **OKLCH** with equal lightness, so no category visually dominates because of its hue, and it is checked for color-blind safety. One accent, **amber** (`#e3a64c` dark / `#9a5d0a` light; decided 2026-11-17), for focus and selection only.
- **The treemap is the hero**, and it gets real design work:
  - Squarified layout (Bruls, Huizing & van Wijk) computed on a background thread, depth-limited by pixel size (rectangles under ~3 px are merged into their parent).
  - **Rendered as a cached bitmap** (measured in the spike): the rectangles are rasterized on the CPU (about 5 ms for 144k) and redrawn only when the layout, size or zoom changes. Labels, hover, selection and the zoom animation are drawn live on top. Painting each rectangle every frame cost 124 ms at that scale.
  - **SpaceSniffer-style folder header strips are the default** (decided 2026-11-17), so the hierarchy reads without hovering. Flat nesting and cushion shading are options in settings.
  - Labels only where they fit, truncated in the middle (`very_long…name.mkv`) and showing size.
  - Zoom transitions of 120–160 ms with ease-out that animate rectangles from their old positions to their new ones. That is the one place motion helps you keep your place, so it is the one place it is used.
- **Size bars in the tree** are thin (3 px), tinted by the dominant file type of that folder, so the tree and treemap share one visual language.
- **Native window integration on Windows 11:** a custom title bar holding the breadcrumb, with a **Mica** backdrop on the title bar and sidebar only (via `window-vibrancy`, or GPUI's own window options). Snap layouts and dark/light follow the system. Content areas stay opaque for legibility and speed.
- **Themes as data:** one TOML token file (the Halloy and Zed approach), shipping one dark and one light theme first. Users can drop in their own.
- **Icons:** a single consistent line-icon set (Lucide or Phosphor, as SVGs rendered by the framework) at 16 px. File-type glyphs come from the same set, not from the Windows shell, to keep the look consistent and avoid slow shell calls.

A clickable mock-up exists, covering the main window in dark and light, network shares with Connect as…, and the design tokens. It is private to the project owner for now.

---

## 6. Phases

| Phase | Scope | Exit criteria |
|---|---|---|
| **0. Spike** (1 week) ✅ | GPUI vs iced prototype; MFT read benchmark using `ntfs-reader` vs a custom parser | Framework chosen (GPUI). Win32 enumeration measured. The MFT run (needs an elevated shell) and on-display frame rates are still open; see [spikes/RESULTS.md](spikes/RESULTS.md) |
| **1. Core** (mostly ✅ in 0.1.0) | `diskie-model`, Win32 fast enumerator, MFT backend, CLI with `scan` and a `top` listing; release pipeline (GitHub Actions builds, minisign-signed files, `latest.json`, see §4.9) | CLI scans C: and a UNC path; meets the scan targets in §1; fuzz and property tests on the model; a tagged build publishes a signed release |
| **2. App MVP** (✅ 0.1.0, minus frame-time checks at 10M nodes) | Window, sidebar (drives), virtual tree-table, treemap, sync between them, live scan progress, Recycle Bin delete, dark and light themes, in-app updater (check, verify, replace, roll back) | Daily-driveable on local disks; frame-time targets met with 10M nodes; one build updates itself to the next |
| **3. Network** | Share discovery, Connect as…, multi-root scans, throttling; benchmark native SMB crates; integrate the better one | Scan a share on a real NAS and on Windows Server; credentials go only to Credential Manager |
| **4. Analysis** | File types panel, top-N, age histogram, owners, search and filters, duplicates | Each view handles 10M nodes without UI stalls |
| **5. History and reports** | Snapshots, compare, CSV/JSON/XLSX/HTML export, scheduled scans via CLI | A snapshot round-trip is lossless; the diff view is correct on fixture trees |
| **6. Polish and ship** | Explorer context-menu entry, USN incremental refresh, settings, portable build + MSI, Authenticode code signing | Signed release, under 20 MB |
| **7. Linux** | POSIX enumerator, Filelight-style unprivileged scanning, built-in SMB client on Linux, **tar.gz and AppImage** packages built in CI (Ubuntu 22.04 base for broad glibc compatibility), Wayland and X11 tested | AppImage runs on a stock Ubuntu, Fedora and Arch desktop without installing anything |

### Testing and benchmarks throughout
- `xtask gen-fixture` builds synthetic trees (deep, wide, with many hardlinks, junction loops, long paths, Unicode names, cloud placeholders) on a VHDX so results are reproducible.
- Criterion benchmarks for every backend run in CI, with results compared against WizTree / TreeSize / pdu timings on the same fixture.
- A test Samba container plus a Windows Server VM for SMB, including slow-link simulation (added latency) to tune concurrency.

---

## 7. Key crates

| Need | Crate |
|---|---|
| UI | `gpui-kit = "=0.10.0"` (pins GPUI as `gpui-pre` 0.3.12) |
| Windows APIs | `windows` (official Microsoft bindings) |
| MFT / USN | `ntfs-reader`, `usn-journal-rs` (evaluate), else a custom parser |
| SMB | `smb2` or `smb` (evaluate both) |
| Parallelism | `rayon`, `crossbeam-channel` |
| Hashing | `xxhash-rust` (xxh3), `blake3` |
| Snapshots | `rkyv`, `zstd`, `memmap2` |
| Export | `rust_xlsxwriter`, `csv`, `serde_json`, `krilla` (PDF) |
| Updates | `ureq` (rustls), `minisign-verify`, `sha2`, `self-replace`, `semver` |
| Credentials | `keyring` |
| Recycle Bin | `trash` |
| Window effects | `window-vibrancy` (if GPUI doesn't expose Mica directly) |
| CLI | `clap` |
| Errors and logs | `thiserror`, `anyhow` (CLI only), `tracing` |

---

## 8. Decisions

| Topic | Decision (2026-11-17) |
|---|---|
| Platforms | Windows first. Linux in phase 7, shipped as a tar.gz and an AppImage. macOS not planned. |
| License | Open source, dual-licensed MIT or Apache-2.0 (the Rust convention). |
| Name | Diskie. |
| Privileges | Windows: administrator by default, by relaunching through UAC at startup (a declined prompt falls back to a standard-user run). Linux: normal user, Filelight-style (§4.8). |
| PDF reports | In v1, built with `krilla` (§4.6). |
| Look | Amber accent; treemap with folder header strips by default. |
| Repository | Hosted on GitHub. |

---

## Sources

- GPUI Kit (formerly gpui-component): https://github.com/longbridge/gpui-component
- Zed Windows release and DirectX 11 renderer: https://zed.dev/blog/windows-progress-report
- awesome-gpui (nohrs, Zedis, zqlz and others): https://github.com/zed-industries/awesome-gpui
- iced 0.14 release notes: https://github.com/iced-rs/iced/releases/tag/0.14.0
- Halloy: https://github.com/squidowl/halloy
- Rerun `re_ui` design tokens: https://docs.rs/crate/re_ui/latest
- Krokiet (Czkawka Slint UI): https://docs.rs/crate/krokiet/latest
- Freya 0.4: https://docs.freyaui.dev/posts/0.4
- Slint royalty-free license (attribution requirement): https://slint.dev/terms-and-conditions
- `smb` (smb-rs) crate: https://docs.rs/crate/smb/latest
- `smb2` crate: https://docs.rs/crate/smb2/latest
- `ntfs-reader`: https://docs.rs/crate/ntfs-reader/latest
- `usn-journal-rs`: https://docs.rs/usn-journal-rs
- parallel-disk-usage benchmarks: https://github.com/KSXGitHub/parallel-disk-usage
- `window-vibrancy`: https://docs.rs/window-vibrancy/latest
- WizTree's MFT approach: https://diskanalyzer.com/about
