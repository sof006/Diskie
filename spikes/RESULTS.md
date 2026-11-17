# Week-1 spike results

The goal was to choose the UI framework from measured numbers rather than reputation, and to check how fast a volume can be enumerated. The test drew the same view in **GPUI** (via GPUI Kit 0.10, `gpui-pre` 0.3.12) and **iced 0.14**: the Diskie main window with every row of a 2,000,010-node tree in a virtualized list, and a treemap of the whole tree.

**Decision: GPUI.** Both frameworks render fast enough once the treemap is cached as a bitmap. GPUI starts 4–10× faster and uses about a third of the memory, and its list virtualization and widget kit come built in.

## Test machine and caveats

- AMD Ryzen 7 5700G, 47 GB RAM, NVIDIA RTX 3060 plus Radeon iGPU, Windows Server 2025. C: is an NVMe SSD; E: is a SATA hard disk.
- **The Windows session was disconnected** (no display attached) during the runs. In that state the desktop compositor presents at about 4 Hz, so GPUI's vsync-locked frames arrived every 250 ms. iced (wgpu) did not wait for vsync and spun at ~1,150 frames/s.
  - Frame *rates* from these runs are therefore meaningless.
  - The comparison uses **CPU time per frame** instead, which is unaffected.
  - Frame rate and visual quality (text rendering in particular) still need to be checked on a connected display. See "Still to run" below.
- The test process was not elevated, so the MFT path could not run here.

## What was measured

`--bench` scrolls the tree about 41 rows per frame and moves the treemap hover highlight to a different rectangle every frame for 10 s. Both prototypes share the same data, layout and bench code (`spike-data`). The treemap used a stress setting with about 144k rectangles. The readable setting a real app would use gives about 3k.

| | GPUI, quads | **GPUI, cached bitmap** | iced, cached mesh | iced, cached bitmap | iced, software renderer |
|---|---|---|---|---|---|
| Treemap technique | 144k `paint_quad` per frame | One bitmap (re-rasterized only on resize); labels and hover live | 144k rectangles tessellated once into a GPU mesh | Bitmap in a cached canvas layer | Same as previous, `tiny-skia` CPU renderer |
| CPU per frame | **124 ms** ✗ | **2.7–2.9 ms** ✓ | ~1.1 ms ✓ | ~1.0 ms ✓ | 11.1 ms ✗ |
| Start to first frame | 246 ms | **260 ms** | 2,811 ms ✗ (tessellation stall) | 1,100 ms | 210 ms |
| Peak memory, 2M nodes loaded | 258 MB | **120 MB** | 385 MB | 335 MB | 106 MB |
| Baseline memory (1,000-node tree) | — | **40 MB** | 240 MB | 240 MB | 25 MB |
| Idle CPU | — | 0 % | — | 0 % | — |
| Release binary | — | 14.8 MB | — | 14.7 MB | — |

The binaries were built with debug line tables; the `dist` profile (fat LTO, stripped) will be smaller.

Other numbers:
- **Data:** generating the 2M-node tree, flattening it and laying out the treemap together takes 110 ms (490 ms for 10M nodes). Rasterizing the 144k-rectangle treemap at 976×320 takes 5 ms.
- **GPUI without the treemap:** the 2M-row list, header and ~390 treemap labels together cost 2.5 ms per frame; labels alone are negligible.
- **Readable treemap (3k rectangles):** GPUI 2.7 ms per frame and 230 ms to first frame; iced 1.0 ms and 1,060 ms.

### Findings

1. **The treemap must be a cached bitmap, in either framework.** At ~0.85 µs per quad, GPUI cannot repaint 144k rectangles every frame. iced's mesh can, but it stalls for about 2.5 s every time it re-tessellates, which happens on each resize, zoom or new scan. A CPU rasterizer (`spike_data::treemap::rasterize`, about 30 lines) fills 144k axis-aligned rectangles in about 5 ms. The frame then draws one image, with labels, hover and selection painted live on top.
2. **Per-frame cost is not the deciding factor.** iced is cheaper per frame (about 1 ms against 2.8 ms), but both are far below the 8 ms budget for 120 Hz.
3. **Startup and memory decide it.** iced on wgpu takes about 1 s to its first frame and holds about 240 MB before any data is loaded. GPUI takes about 0.25 s and 40 MB, which meets the plan's targets of under 200 ms and under 40 MB idle. iced's software renderer is light but too slow per frame (11 ms at only 1000×720).
4. **Ergonomics favour GPUI as well.** `uniform_list` virtualized 2M rows with no extra code. In iced I had to build virtualization by hand from spacers and the scroll offset. GPUI Kit also adds the table, tree, dock, command palette and inputs we would otherwise write ourselves.
5. **Risk accepted:** GPUI is pre-1.0. GPUI Kit pins one `gpui-pre` release, and the app will pin GPUI Kit exactly (`=0.10.0`) and upgrade on purpose.

## Volume enumeration (no admin)

`spike-scan win32` uses parallel `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH`; `spike-scan std` uses parallel `std::fs::read_dir`. Junctions and symlinks are not followed.

| Volume | Entries | Mode | Time | Throughput | Peak RAM |
|---|---|---|---|---|---|
| C: (NVMe), warm cache | 670k (497k files, 80 GB) | win32 | 3.1–3.8 s | 177–214k/s | 26 MB |
| C: (NVMe), warm cache | 667k | std | 2.4–2.5 s | 268–284k/s | 25 MB |
| E: (hard disk), **cold cache** | 1.68M | win32 | **did not finish in 300 s** | — | — |
| E: (hard disk), partly warm | 1.68M | std | 103 s | 16k/s | 8 MB |
| E: (hard disk), warm cache | 1.68M (1.53M files, 851 GB) | win32 | 2.3 s | 718k/s | 9 MB |
| E: (hard disk), warm cache | 1.68M | std | 2.7 s | 621k/s | 9 MB |

### Findings

6. **A highly parallel directory walk is catastrophic on a cold hard disk.** Every worker seeks somewhere different, so it ran at least 100× slower than the same walk with a warm cache. The production scanner must detect the drive type (`IOCTL_STORAGE_QUERY_PROPERTY`, seek penalty) and cap concurrency at 1–2 on hard disks. Reading the MFT, which is one sequential read, matters most exactly there.
7. Unelevated, C: still has 118 folders it cannot read. That confirms the plan's "Unreadable" panel and the admin-by-default decision.
8. `std::fs::read_dir` already performs well on Windows; the hand-written `FindFirstFileExW` walker only won on E: with a warm cache. The production backend should move to `NtQueryDirectoryFile` with larger buffers and a work-stealing queue instead of one rayon task per folder, and be benchmarked again.

## Still to run (needs you)

1. **On a connected display**, for real frame rates and a visual check of text and colours:

   ```bash
   cargo build --release -p spike-gpui -p spike-iced
   ```

   ```bash
   target/release/spike-gpui.exe --bench
   ```

   ```bash
   target/release/spike-iced.exe --bench
   ```

   Run them without `--bench` to scroll and hover by hand. The results go to `spike-results/`.

2. **MFT read, from an elevated terminal** (run as administrator):

   ```bash
   target/release/spike-scan.exe mft C:
   ```

   ```bash
   target/release/spike-scan.exe mft E:
   ```

   Compare these against the Win32 numbers above, especially for the hard disk E:.

## Reproducing

The environment variables are:

| Variable | Meaning |
|---|---|
| `SPIKE_NODES` | Tree size (default 2,000,000) |
| `SPIKE_MAP_MODE` | GPUI: `raster` (default) or `quads`. iced: `mesh` (default) or `raster` |
| `SPIKE_PAD`, `SPIKE_MIN_GROUP`, `SPIKE_MIN_AREA` | Treemap density. The default is the stress setting; `1 / 22 / 3` is readable |
| `SPIKE_LABELS=0` | Turn treemap labels off |
| `SPIKE_BENCH_SECS` | Benchmark length in seconds |
| `SPIKE_TAG` | Suffix for the result file name |
| `ICED_BACKEND=tiny-skia` | Use iced's software renderer |

The raw results are the JSON files in [`spike-results/`](../spike-results).
