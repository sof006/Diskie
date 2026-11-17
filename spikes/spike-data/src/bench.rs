//! Scripted benchmark run and statistics shared by both prototypes.
//!
//! With `--bench`, a prototype scrolls the 2M-row tree fast and sweeps the
//! hover highlight across the treemap on every frame for `SPIKE_BENCH_SECS`
//! (default 10), records frame intervals and its own per-frame build time,
//! samples process CPU and memory, then writes a JSON report and exits.

use std::time::{Duration, Instant};

pub struct BenchDriver {
    start: Option<Instant>,
    duration: Duration,
    frame: u64,
    rows: usize,
    rects: usize,
}

pub struct BenchStep {
    pub scroll_row: usize,
    pub hover_rect: usize,
}

impl BenchDriver {
    /// Some when the process was started with `--bench`.
    pub fn from_args(rows: usize, rects: usize) -> Option<Self> {
        if !std::env::args().any(|a| a == "--bench") {
            return None;
        }
        let secs = std::env::var("SPIKE_BENCH_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10.0);
        Some(Self { start: None, duration: Duration::from_secs_f64(secs), frame: 0, rows, rects })
    }

    /// The next frame's scripted input, or None once the run is over.
    pub fn step(&mut self) -> Option<BenchStep> {
        let start = *self.start.get_or_insert_with(Instant::now);
        if start.elapsed() >= self.duration {
            return None;
        }
        self.frame += 1;
        Some(BenchStep {
            // ~40 rows per frame: a fast flick-scroll through the whole tree.
            scroll_row: (self.frame as usize * 41) % self.rows.max(1),
            hover_rect: (self.frame as usize * 7919) % self.rects.max(1),
        })
    }
}

#[derive(Default)]
pub struct FrameStats {
    last: Option<Instant>,
    pub intervals_ms: Vec<f32>,
    pub build_ms: Vec<f32>,
}

impl FrameStats {
    pub fn frame(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last.replace(now) {
            self.intervals_ms.push((now - prev).as_secs_f32() * 1000.0);
        }
    }

    pub fn build(&mut self, d: Duration) {
        self.build_ms.push(d.as_secs_f32() * 1000.0);
    }

    /// Short live readout for the prototype's header.
    pub fn live(&self) -> String {
        let tail = |v: &[f32]| -> f32 {
            let n = v.len().min(60);
            if n == 0 { 0.0 } else { v[v.len() - n..].iter().sum::<f32>() / n as f32 }
        };
        let iv = tail(&self.intervals_ms);
        let fps = if iv > 0.0 { 1000.0 / iv } else { 0.0 };
        format!("{fps:.0} fps · frame build {:.2} ms", tail(&self.build_ms))
    }
}

#[derive(Clone, Copy)]
pub struct ProcSample {
    pub at: Instant,
    pub cpu_ms: f64,
    pub working_set: u64,
    pub peak_working_set: u64,
}

impl ProcSample {
    #[cfg(windows)]
    pub fn now() -> Self {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let ft = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (ft(), ft(), ft(), ft());
        let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        unsafe {
            let h = GetCurrentProcess();
            GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
            GetProcessMemoryInfo(h, &mut mem, mem.cb);
        }
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        Self {
            at: Instant::now(),
            cpu_ms: t(k) + t(u),
            working_set: mem.WorkingSetSize as u64,
            peak_working_set: mem.PeakWorkingSetSize as u64,
        }
    }

    #[cfg(not(windows))]
    pub fn now() -> Self {
        Self { at: Instant::now(), cpu_ms: 0.0, working_set: 0, peak_working_set: 0 }
    }
}

pub struct BenchReport<'a> {
    pub framework: &'a str,
    pub nodes: usize,
    pub rows: usize,
    pub rects: usize,
    pub data_build_ms: f64,
    pub startup_ms: f64,
    pub stats: &'a FrameStats,
    pub begin: ProcSample,
    pub end: ProcSample,
}

fn pct(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[((s.len() - 1) as f32 * p).round() as usize]
}

impl BenchReport<'_> {
    /// Prints the report and writes `spike-results/<framework>.json` next to the workspace root.
    pub fn write(&self) {
        let iv = &self.stats.intervals_ms;
        let bd = &self.stats.build_ms;
        let wall = (self.end.at - self.begin.at).as_secs_f64() * 1000.0;
        let cpu_pct = (self.end.cpu_ms - self.begin.cpu_ms) / wall.max(1.0) * 100.0;
        let mean = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
        let over = |v: &[f32], ms: f32| v.iter().filter(|x| **x > ms).count();
        let json = format!(
            r#"{{
  "framework": "{fw}",
  "nodes": {nodes},
  "rows": {rows},
  "treemap_rects": {rects},
  "data_build_ms": {db:.1},
  "startup_to_first_frame_ms": {st:.1},
  "frames": {frames},
  "interval_ms": {{ "mean": {im:.2}, "p50": {i50:.2}, "p95": {i95:.2}, "p99": {i99:.2}, "max": {imax:.2} }},
  "frames_over_16_7ms": {o16},
  "frames_over_33ms": {o33},
  "build_ms": {{ "mean": {bm:.2}, "p50": {b50:.2}, "p95": {b95:.2}, "p99": {b99:.2}, "max": {bmax:.2} }},
  "process_cpu_percent_of_one_core": {cpu:.1},
  "process_cpu_ms_per_frame": {cpf:.2},
  "working_set_mb_end": {ws:.1},
  "peak_working_set_mb": {pk:.1}
}}
"#,
            fw = self.framework,
            nodes = self.nodes,
            rows = self.rows,
            rects = self.rects,
            db = self.data_build_ms,
            st = self.startup_ms,
            frames = iv.len() + 1,
            im = mean(iv),
            i50 = pct(iv, 0.5),
            i95 = pct(iv, 0.95),
            i99 = pct(iv, 0.99),
            imax = pct(iv, 1.0),
            o16 = over(iv, 17.5),
            o33 = over(iv, 34.0),
            bm = mean(bd),
            b50 = pct(bd, 0.5),
            b95 = pct(bd, 0.95),
            b99 = pct(bd, 0.99),
            bmax = pct(bd, 1.0),
            cpu = cpu_pct,
            cpf = (self.end.cpu_ms - self.begin.cpu_ms) / (iv.len().max(1) as f64),
            ws = self.end.working_set as f64 / 1e6,
            pk = self.end.peak_working_set as f64 / 1e6,
        );
        print!("{json}");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spike-results");
        let _ = std::fs::create_dir_all(&dir);
        let tag = std::env::var("SPIKE_TAG").map(|t| format!("-{t}")).unwrap_or_default();
        let _ = std::fs::write(dir.join(format!("{}{tag}.json", self.framework)), json);
    }
}
