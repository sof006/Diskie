//! The main window.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use diskie_model::tree::flag;
use diskie_model::{KIND_COUNT, Kind, NONE, Tree, fmt};
use diskie_scan::volumes::{self, Volume, VolumeKind};
use diskie_scan::{Backend, Phase, Progress};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons;
use crate::settings::{self, Settings, UpdateMode};
use crate::theme::Theme;
use crate::treemap_view::{self, MapState, Paint};

const ROW_H: f32 = 24.0;
const TOOLBAR_H: f32 = 40.0;
const STATUS_H: f32 = 26.0;
const SIDEBAR_W: f32 = 232.0;
const TYPES_W: f32 = 264.0;
const MAP_BAR_H: f32 = 26.0;

pub struct Loaded {
    pub tree: Arc<Tree>,
    pub root: PathBuf,
    pub backend: Backend,
    pub elapsed: Duration,
    pub fallback: Option<String>,
    pub space: Option<(u64, u64)>,
    expanded: Vec<bool>,
    rows: Vec<u32>,
    selected: Option<u32>,
    hovered: Option<u32>,
    zoom: u32,
    generation: u64,
    kind_totals: Option<(u32, u64, [u64; KIND_COUNT])>,
}

impl Loaded {
    fn new(scan: diskie_scan::Scan, root: PathBuf) -> Self {
        let n = scan.tree.len();
        let mut expanded = vec![false; n];
        expanded[Tree::ROOT as usize] = true;
        let space = volumes::space(&root);
        let mut l = Loaded {
            tree: Arc::new(scan.tree),
            root,
            backend: scan.backend,
            elapsed: scan.elapsed,
            fallback: scan.fallback,
            space,
            expanded,
            rows: Vec::new(),
            selected: Some(Tree::ROOT),
            hovered: None,
            zoom: Tree::ROOT,
            generation: 0,
            kind_totals: None,
        };
        l.rebuild_rows();
        l
    }

    fn rebuild_rows(&mut self) {
        let t = &self.tree;
        self.rows.clear();
        let mut stack = vec![Tree::ROOT];
        while let Some(n) = stack.pop() {
            self.rows.push(n);
            if self.expanded[n as usize] {
                stack.extend(t.kids(n).iter().rev());
            }
        }
    }

    fn reveal(&mut self, n: u32) -> Option<usize> {
        let mut changed = false;
        let mut a = self.tree.parent[n as usize];
        while a != NONE {
            if !self.expanded[a as usize] {
                self.expanded[a as usize] = true;
                changed = true;
            }
            a = self.tree.parent[a as usize];
        }
        if changed {
            self.rebuild_rows();
        }
        self.rows.iter().position(|&r| r == n)
    }

    fn kind_totals(&mut self) -> [u64; KIND_COUNT] {
        match self.kind_totals {
            Some((z, g, k)) if z == self.zoom && g == self.generation => k,
            _ => {
                let k = self.tree.kind_totals(self.zoom);
                self.kind_totals = Some((self.zoom, self.generation, k));
                k
            }
        }
    }
}

enum ScanState {
    Idle,
    Running { root: PathBuf, progress: Arc<Progress>, started: Instant },
    Failed { root: PathBuf, error: String },
}

#[derive(Clone)]
enum UpdateState {
    Idle,
    Checking { manual: bool },
    UpToDate,
    Available(diskie_update::Update),
    Downloading { version: String, done: u64, total: u64 },
    /// `exe` is where the new version now lives (renamed when it carried the old version's name).
    Installed { version: String, exe: PathBuf },
    Failed(String),
}

struct Menu {
    at: Point<Pixels>,
    node: u32,
}

pub struct Diskie {
    theme: Theme,
    settings: Settings,
    volumes: Vec<Volume>,
    elevated: bool,
    scan: ScanState,
    loaded: Option<Loaded>,
    focus: FocusHandle,
    list: UniformListScrollHandle,
    map: Rc<RefCell<MapState>>,
    map_job: Option<Task<()>>,
    menu: Option<Menu>,
    dragging_split: bool,
    update: UpdateState,
    notice: Option<(String, bool)>,
    _tasks: Vec<Task<()>>,
}

impl Diskie {
    pub fn new(start: Option<PathBuf>, startup_notice: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let dark = is_dark(window);
        let settings = Settings::load();
        let mut this = Self {
            theme: if dark { Theme::DARK } else { Theme::LIGHT },
            settings,
            volumes: volumes::list_volumes(),
            elevated: volumes::is_elevated(),
            scan: ScanState::Idle,
            loaded: None,
            focus,
            list: UniformListScrollHandle::new(),
            map: Rc::default(),
            map_job: None,
            menu: None,
            dragging_split: false,
            update: UpdateState::Idle,
            notice: startup_notice.map(|n| (n, false)),
            _tasks: Vec::new(),
        };
        let obs = cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
            let dark = is_dark(window);
            this.theme = if dark { Theme::DARK } else { Theme::LIGHT };
            cx.notify();
        });
        obs.detach();
        if let Some(p) = start {
            this.start_scan(p, window, cx);
        }
        this.schedule_update_check(cx);
        this
    }

    // ------------------------------------------------------------------------------------
    // Scanning

    fn start_scan(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let ScanState::Running { progress, .. } = &self.scan {
            progress.cancel();
        }
        let progress = Arc::new(Progress::default());
        self.scan = ScanState::Running { root: root.clone(), progress: progress.clone(), started: Instant::now() };
        self.menu = None;
        self.notice = None;
        window.set_window_title(&format!("Scanning {} · Diskie", root.display()));
        let opts = diskie_scan::Options { mft: self.settings.use_mft, threads: None };
        let (p2, r2) = (progress.clone(), root.clone());
        let job = cx.background_spawn(async move { crate::crash::guard(|| diskie_scan::scan(&r2, opts, &p2)) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Ignore results from a scan that was replaced by a newer one.
                let current = matches!(&this.scan, ScanState::Running { progress: p, .. } if Arc::ptr_eq(p, &progress));
                if !current {
                    return;
                }
                let result = result.unwrap_or_else(|internal| Err(diskie_scan::Error::Internal(internal)));
                match result {
                    Ok(scan) => {
                        window.set_window_title(&format!("{} · Diskie", root.display()));
                        this.loaded = Some(Loaded::new(scan, root.clone()));
                        this.scan = ScanState::Idle;
                        this.list.scroll_to_item(0, ScrollStrategy::Top);
                        this.map.borrow_mut().pending = None;
                        this.volumes = volumes::list_volumes();
                    }
                    Err(diskie_scan::Error::Cancelled) => {
                        this.scan = ScanState::Idle;
                        window.set_window_title("Diskie");
                    }
                    Err(e) => {
                        this.scan = ScanState::Failed { root: root.clone(), error: e.to_string() };
                        window.set_window_title("Diskie");
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        // Repaint the live counters ten times a second while the scan runs.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.scan, ScanState::Running { .. })
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        });
        self._tasks.push(ticker);
        self._tasks.retain(|_| true);
        cx.notify();
    }

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        if let ScanState::Running { progress, .. } = &self.scan {
            progress.cancel();
        }
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Scan this folder".into()),
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| this.start_scan(p, window, cx));
            }
        });
        self._tasks.push(task);
    }

    fn rescan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = match (&self.scan, &self.loaded) {
            (ScanState::Failed { root, .. }, _) => Some(root.clone()),
            (_, Some(l)) => Some(l.root.clone()),
            _ => None,
        };
        if let Some(r) = root {
            self.start_scan(r, window, cx);
        }
    }

    // ------------------------------------------------------------------------------------
    // Navigation and selection

    fn select(&mut self, n: u32, reveal: bool, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        l.selected = Some(n);
        if reveal && let Some(i) = l.reveal(n) {
            self.list.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn toggle(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if l.tree.kids(n).is_empty() {
            return;
        }
        l.expanded[n as usize] = !l.expanded[n as usize];
        l.rebuild_rows();
        cx.notify();
    }

    fn zoom_to(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        let target = if l.tree.is_dir(n) { n } else { l.tree.parent[n as usize] };
        if target == NONE {
            return;
        }
        l.zoom = target;
        l.expanded[target as usize] = true;
        if let Some(i) = l.reveal(target) {
            l.selected = Some(target);
            self.list.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn zoom_out(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_ref() else { return };
        let p = l.tree.parent[l.zoom as usize];
        if p != NONE {
            let z = l.zoom;
            self.zoom_to(p, cx);
            self.select(z, true, cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_ref() else { return };
        let cur = l.selected.and_then(|s| l.rows.iter().position(|&r| r == s)).unwrap_or(0);
        let next = (cur as isize + delta).clamp(0, l.rows.len() as isize - 1) as usize;
        let n = l.rows[next];
        self.select(n, true, cx);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let ctrl = k.modifiers.control || k.modifiers.platform;
        match k.key.as_str() {
            "f5" => self.rescan(window, cx),
            "o" if ctrl => self.pick_folder(window, cx),
            "escape" => {
                if self.menu.take().is_some() {
                    cx.notify();
                } else if matches!(self.scan, ScanState::Running { .. }) {
                    self.cancel_scan(cx);
                }
            }
            _ => {}
        }
        let Some(l) = self.loaded.as_ref() else { return };
        let sel = l.selected.unwrap_or(Tree::ROOT);
        match k.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(20, cx),
            "pageup" => self.move_selection(-20, cx),
            "home" => self.select(Tree::ROOT, true, cx),
            "right" => {
                if !l.expanded[sel as usize] && !l.tree.kids(sel).is_empty() {
                    self.toggle(sel, cx);
                } else if let Some(&first) = l.tree.kids(sel).first() {
                    self.select(first, true, cx);
                }
            }
            "left" => {
                if l.expanded[sel as usize] && sel != Tree::ROOT {
                    self.toggle(sel, cx);
                } else if l.tree.parent[sel as usize] != NONE {
                    let p = l.tree.parent[sel as usize];
                    self.select(p, true, cx);
                }
            }
            "enter" => self.zoom_to(sel, cx),
            "backspace" => self.zoom_out(cx),
            "delete" => self.recycle(sel, window, cx),
            "c" if ctrl => self.copy_path(sel, cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------
    // Actions on files

    fn copy_path(&mut self, n: u32, cx: &mut Context<Self>) {
        if let Some(l) = &self.loaded {
            cx.write_to_clipboard(ClipboardItem::new_string(l.tree.path(n)));
            self.notice = Some(("Path copied".into(), false));
            cx.notify();
        }
    }

    fn reveal_in_explorer(&mut self, n: u32, cx: &mut Context<Self>) {
        if let Some(l) = &self.loaded {
            cx.reveal_path(Path::new(&l.tree.path(n)));
        }
    }

    fn recycle(&mut self, n: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        if n == Tree::ROOT || l.tree.has(n, flag::SYNTHETIC) {
            return;
        }
        let path = l.tree.path(n);
        if !volumes::has_recycle_bin(Path::new(&path)) {
            self.notice = Some((
                "This location has no Recycle Bin (network share or removable drive), so Diskie won't delete here. Use Explorer if you mean to delete it permanently.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let name = l.tree.name(n).to_string();
        let mut detail = if l.tree.is_dir(n) {
            format!("{}, {} files in {} folders.\n{path}", fmt::size(l.tree.size[n as usize]), fmt::count(l.tree.files[n as usize] as u64), fmt::count(l.tree.dirs[n as usize] as u64))
        } else {
            format!("{}.\n{path}", fmt::size(l.tree.size[n as usize]))
        };
        if is_system_path(&path) {
            detail.push_str("\n\nThis is inside a Windows system folder. Removing it can stop Windows or installed programs from working.");
        }
        let answer = window.prompt(PromptLevel::Warning, &format!("Move “{name}” to the Recycle Bin?"), Some(&detail), &["Move to Recycle Bin", "Cancel"], cx);
        self.menu = None;
        let task = cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let p2 = path.clone();
            let result = cx.background_spawn(async move { trash::delete(&p2) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if let Some(l) = this.loaded.as_mut() {
                            let tree = Arc::make_mut(&mut l.tree);
                            let freed = tree.size[n as usize];
                            let parent = tree.parent[n as usize];
                            tree.remove(n);
                            l.generation += 1;
                            if l.tree.is_ancestor(n, l.zoom) {
                                l.zoom = parent;
                            }
                            l.selected = Some(parent);
                            l.rebuild_rows();
                            l.space = volumes::space(&l.root);
                            this.notice = Some((format!("Moved {name} to the Recycle Bin ({})", fmt::size(freed)), false));
                        }
                    }
                    Err(e) => this.notice = Some((format!("Could not move {name} to the Recycle Bin: {e}"), true)),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------------------------
    // Updates

    fn schedule_update_check(&mut self, cx: &mut Context<Self>) {
        if self.settings.updates == UpdateMode::Off || settings::now().saturating_sub(self.settings.last_update_check) < 20 * 3600 {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| this.check_for_updates(false, cx));
        });
        self._tasks.push(task);
    }

    fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.update, UpdateState::Checking { .. } | UpdateState::Downloading { .. }) {
            return;
        }
        self.update = UpdateState::Checking { manual };
        self.settings.last_update_check = settings::now();
        self.settings.save();
        cx.notify();
        let job = cx.background_spawn(async move { diskie_update::check(env!("CARGO_PKG_VERSION")) });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                this.update = match r {
                    Ok(Some(u)) => UpdateState::Available(u),
                    Ok(None) if manual => UpdateState::UpToDate,
                    Ok(None) => UpdateState::Idle,
                    Err(e) if manual => UpdateState::Failed(e.to_string()),
                    Err(_) => UpdateState::Idle,
                };
                let auto = this.settings.updates == UpdateMode::Auto;
                if auto && let UpdateState::Available(u) = this.update.clone() {
                    this.install_update(u, cx);
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn install_update(&mut self, u: diskie_update::Update, cx: &mut Context<Self>) {
        if let Err(e) = diskie_update::can_self_update() {
            self.update = UpdateState::Failed(e.to_string());
            cx.notify();
            return;
        }
        let version = u.version.to_string();
        self.update = UpdateState::Downloading { version: version.clone(), done: 0, total: u.asset.size };
        cx.notify();
        let progress = Arc::new((std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicBool::new(false)));
        let p2 = progress.clone();
        let v2 = version.clone();
        let job = cx.background_spawn(async move {
            let r = diskie_update::download(&u, |done, _| p2.0.store(done, Relaxed))
                .and_then(|file| diskie_update::install(&file, env!("CARGO_PKG_VERSION"), &v2));
            p2.1.store(true, Relaxed);
            r
        });
        let ticker = cx.spawn(async move |this, cx| {
            while !progress.1.load(Relaxed) {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let done = progress.0.load(Relaxed);
                let _ = this.update(cx, |this, cx| {
                    if let UpdateState::Downloading { done: d, .. } = &mut this.update {
                        *d = done;
                        cx.notify();
                    }
                });
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                this.update = match r {
                    Ok(exe) => UpdateState::Installed { version, exe },
                    Err(e) => UpdateState::Failed(e.to_string()),
                };
                cx.notify();
            });
        });
        self._tasks.push(ticker);
        self._tasks.push(task);
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        // After an update, start the file where the new version now is: Windows keeps reporting
        // this process's original path even if the update renamed the file.
        let exe = match &self.update {
            UpdateState::Installed { exe, .. } => Ok(exe.clone()),
            _ => diskie_update::current_exe(),
        };
        if let Ok(exe) = exe {
            let mut cmd = std::process::Command::new(exe);
            // Restart with the same rights: an elevated copy starts elevated, and a standard-user
            // copy (prompt declined, or turned off) doesn't ask again.
            if !self.elevated {
                cmd.arg(crate::elevate::NO_ELEVATE_ARG);
            }
            if let Some(l) = &self.loaded {
                cmd.arg(&l.root);
            }
            if cmd.spawn().is_ok() {
                cx.quit();
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // Treemap layout scheduling

    fn schedule_map(&mut self, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        let wanted = self.map.borrow().wanted(l.zoom, l.generation, self.theme.dark);
        let Some(key) = wanted else { return };
        {
            let m = self.map.borrow();
            if m.current.as_ref().is_some_and(|c| c.key == key) || m.pending == Some(key) || m.failed == Some(key) {
                return;
            }
            if m.pending.is_some() {
                return; // one job at a time; the next render asks again
            }
        }
        self.map.borrow_mut().pending = Some(key);
        let (tree, palette) = (l.tree.clone(), self.theme.palette());
        let job = cx.background_spawn(async move { crate::crash::guard(|| treemap_view::build(tree, key, palette)) });
        self.map_job = Some(cx.spawn(async move |this, cx| {
            let built = job.await;
            let _ = this.update(cx, |this, cx| {
                let mut m = this.map.borrow_mut();
                match built {
                    Ok(built) => m.install(built),
                    Err(e) => {
                        // Don't ask for the same layout again on the next frame.
                        m.failed = Some(key);
                        this.notice = Some((format!("Couldn't draw the treemap. {e}"), true));
                    }
                }
                m.pending = None;
                drop(m);
                cx.notify();
            });
        }));
    }

    // ------------------------------------------------------------------------------------
    // Rendering

    fn icon(&self, name: &'static str, color: u32) -> Svg {
        svg().path(icons::path(name)).size(px(16.)).flex_none().text_color(rgb(color))
    }

    fn button(&self, id: &'static str, icon: &'static str, label: &'static str) -> Stateful<Div> {
        let t = self.theme;
        div()
            .id(id)
            .h(px(28.))
            .px_2p5()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(t.btn_line))
            .text_color(rgb(t.text))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(t.hover)))
            .child(self.icon(icon, t.muted))
            .when(!label.is_empty(), |d| d.child(label))
    }

    fn toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let mut crumbs = div().flex().items_center().gap_1().min_w_0().overflow_hidden();
        if let Some(l) = &self.loaded {
            let lineage = l.tree.lineage(l.zoom);
            let last = lineage.len() - 1;
            for (i, n) in lineage.into_iter().enumerate() {
                if i > 0 {
                    crumbs = crumbs.child(self.icon("chevron-right", t.faint).size(px(12.)));
                }
                let name = l.tree.name(n).to_string();
                crumbs = crumbs.child(
                    div()
                        .id(("crumb", n as usize))
                        .px_1()
                        .rounded(px(4.))
                        .flex_none()
                        .text_color(rgb(if i == last { t.text } else { t.muted }))
                        .when(i == last, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(t.hover)))
                        .child(name)
                        .on_click(cx.listener(move |this, _, _, cx| this.zoom_to(n, cx))),
                );
            }
        } else {
            crumbs = crumbs.child(div().text_color(rgb(t.muted)).child("No scan open"));
        }
        let can_up = self.loaded.as_ref().is_some_and(|l| l.zoom != Tree::ROOT);
        let can_rescan = self.loaded.is_some() || matches!(self.scan, ScanState::Failed { .. });
        div()
            .h(px(TOOLBAR_H))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(t.chrome))
            .border_b_1()
            .border_color(rgb(t.chrome_line))
            .child(
                self.button("up", "arrow-up", "")
                    .when(!can_up, |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx))),
            )
            .child(div().flex_1().min_w_0().child(crumbs))
            .child(
                self.button("rescan", "refresh", "Rescan")
                    .when(!can_rescan, |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, window, cx| this.rescan(window, cx))),
            )
            .child(self.button("open", "folder", "Scan folder or share…").on_click(cx.listener(|this, _, window, cx| this.pick_folder(window, cx))))
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let active_root = self.loaded.as_ref().map(|l| l.root.to_string_lossy().to_string());
        let heading = |s: &'static str| {
            div().px_3().pt_3().pb_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.faint)).child(s)
        };
        let mut list = div().flex().flex_col().child(heading("DRIVES"));
        for (i, v) in self.volumes.iter().enumerate() {
            let used = v.total.saturating_sub(v.free);
            let frac = if v.total > 0 { used as f32 / v.total as f32 } else { 0.0 };
            let active = active_root.as_deref() == Some(v.root.as_str());
            let root = PathBuf::from(&v.root);
            let title = if v.label.is_empty() { v.root.trim_end_matches('\\').to_string() } else { format!("{}  {}", v.root.trim_end_matches('\\'), v.label) };
            let icon = if v.kind == VolumeKind::Network { "server" } else { "drive" };
            list = list.child(
                div()
                    .id(("drive", i))
                    .mx_1p5()
                    .px_2()
                    .py_1p5()
                    .rounded(px(5.))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgba(t.side_active)))
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child(self.icon(icon, if active { t.accent } else { t.muted }).mt(px(1.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().truncate().child(title))
                            .child(
                                div()
                                    .h(px(3.))
                                    .rounded(px(2.))
                                    .bg(rgb(t.track))
                                    .child(div().h_full().rounded(px(2.)).w(relative(frac)).bg(rgb(if frac > 0.9 { t.danger } else { t.muted }))),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(t.muted))
                                    .child(format!("{} free of {}{}", fmt::size(v.free), fmt::size(v.total), if v.filesystem.is_empty() { String::new() } else { format!(" · {}", v.filesystem) })),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.start_scan(root.clone(), window, cx))),
            );
        }
        list = list.child(heading("NETWORK AND FOLDERS")).child(
            div()
                .id("pick")
                .mx_1p5()
                .px_2()
                .py_1p5()
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.hover)))
                .child(self.icon("folder", t.muted))
                .child("Scan folder or share…")
                .on_click(cx.listener(|this, _, window, cx| this.pick_folder(window, cx))),
        );
        list = list.child(
            div()
                .px_3()
                .pt_1()
                .text_size(px(11.))
                .text_color(rgb(t.faint))
                .child(r"Type a \\server\share path into the folder box to scan a network share."),
        );

        let (mode_icon, mode) = if self.elevated {
            ("shield", "Administrator · fast MFT scans")
        } else {
            ("lock", "Standard user · folder walk")
        };
        div()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.chrome))
            .border_r_1()
            .border_color(rgb(t.chrome_line))
            .child(div().id("side-scroll").flex_1().min_h_0().overflow_y_scroll().child(list))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(t.chrome_line))
                    .text_size(px(11.))
                    .text_color(rgb(t.muted))
                    .child(self.icon(mode_icon, t.muted).size(px(14.)))
                    .child(mode),
            )
    }

    fn rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let t = self.theme;
        let Some(l) = &self.loaded else { return Vec::new() };
        let tree = &l.tree;
        let now = settings::now();
        range
            .filter_map(|i| l.rows.get(i).copied())
            .map(|n| {
                let ni = n as usize;
                let is_dir = tree.is_dir(n);
                let has_kids = !tree.kids(n).is_empty();
                let expanded = l.expanded[ni];
                let selected = l.selected == Some(n);
                let hovered = l.hovered == Some(n);
                let pct = tree.percent_of_parent(n);
                let depth = tree.depth[ni] as f32;
                let unreadable = tree.has(n, flag::UNREADABLE);
                let link = tree.has(n, flag::REPARSE);
                let name_color = if unreadable || link { t.muted } else { t.text };
                let chevron = if has_kids {
                    self.icon(if expanded { "chevron-down" } else { "chevron-right" }, t.muted).size(px(14.))
                } else {
                    self.icon("chevron-right", t.muted).size(px(14.)).opacity(0.)
                };
                let glyph = if link {
                    "link"
                } else if is_dir {
                    "folder"
                } else {
                    "file"
                };
                let mut name = div().flex_1().min_w_0().flex().items_center().gap_1p5().pl(px(depth * 16.0)).child(
                    div()
                        .id(("chev", ni))
                        .flex_none()
                        .child(chevron)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select(n, false, cx);
                            this.toggle(n, cx);
                        })),
                );
                name = name
                    .child(self.icon(glyph, if is_dir { t.kind(tree.kind[ni]) } else { t.faint }).size(px(14.)))
                    .child(div().min_w_0().truncate().text_color(rgb(name_color)).child(tree.name(n).to_string()));
                if unreadable {
                    name = name.child(tag("no access", t));
                } else if link {
                    name = name.child(tag("link", t));
                }
                div()
                    .id(("row", ni))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_3()
                    .when(selected, |d| d.bg(rgba(t.selection)))
                    .when(hovered && !selected, |d| d.bg(rgba(t.hover)))
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child(name)
                    .child(num(fmt::size(tree.size[ni]), 76.).text_color(rgb(t.text)))
                    .child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .h(px(3.))
                            .rounded(px(2.))
                            .bg(rgb(t.track))
                            .child(div().h_full().rounded(px(2.)).w(relative((pct / 100.0) as f32)).bg(rgb(t.kind(tree.kind[ni])))),
                    )
                    .child(num(fmt::percent(pct), 52.).text_color(rgb(t.muted)))
                    .child(num(if is_dir { fmt::count(tree.files[ni] as u64) } else { String::new() }, 72.).text_color(rgb(t.muted)))
                    .child(num(fmt::age(tree.mtime[ni], now), 56.).text_color(rgb(t.muted)))
                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                        if let Some(l) = this.loaded.as_mut() {
                            let h = if *hovering { Some(n) } else { None };
                            if l.hovered != h && (*hovering || l.hovered == Some(n)) {
                                l.hovered = h;
                                cx.notify();
                            }
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.menu = None;
                        if ev.click_count >= 2 {
                            if is_dir {
                                this.zoom_to(n, cx);
                            } else {
                                this.reveal_in_explorer(n, cx);
                            }
                        } else {
                            this.select(n, false, cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.select(n, false, cx);
                        this.menu = Some(Menu { at: ev.position, node: n });
                        cx.notify();
                    }))
            })
            .collect()
    }

    fn table(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let count = self.loaded.as_ref().map(|l| l.rows.len()).unwrap_or(0);
        let head = |s: &'static str, w: f32| div().w(px(w)).flex_none().flex().justify_end().child(s);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .gap_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.faint))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child(div().flex_1().pl(px(20.)).child("NAME"))
                    .child(head("SIZE", 76.))
                    .child(div().w(px(96.)).flex_none().child("OF PARENT"))
                    .child(head("", 52.))
                    .child(head("FILES", 72.))
                    .child(head("MODIFIED", 56.)),
            )
            .child(
                uniform_list("tree", count, cx.processor(|this: &mut Self, range, _, cx| this.rows(range, cx)))
                    .track_scroll(&self.list)
                    .flex_1()
                    .min_h_0(),
            )
    }

    fn types_panel(&mut self) -> impl IntoElement + use<> {
        let t = self.theme;
        let mut col = div().flex().flex_col().gap_0p5().px_3().pt_2();
        if let Some(l) = self.loaded.as_mut() {
            let totals = l.kind_totals();
            let all: u64 = totals.iter().sum::<u64>().max(1);
            let mut order: Vec<usize> = (0..KIND_COUNT).filter(|&k| totals[k] > 0).collect();
            order.sort_by_key(|&k| std::cmp::Reverse(totals[k]));
            for k in order {
                let frac = totals[k] as f32 / all as f32;
                col = col.child(
                    div()
                        .py_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size(px(10.)).flex_none().rounded(px(2.)).bg(rgb(t.kind(k as u8))))
                                .child(div().flex_1().min_w_0().truncate().child(Kind::from_u8(k as u8).label()))
                                .child(div().flex_none().child(fmt::size(totals[k]))),
                        )
                        .child(
                            div()
                                .ml(px(18.))
                                .h(px(3.))
                                .rounded(px(2.))
                                .bg(rgb(t.track))
                                .child(div().h_full().rounded(px(2.)).w(relative(frac)).bg(rgb(t.kind(k as u8)))),
                        ),
                );
            }
        }
        div()
            .w(px(TYPES_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(t.line))
            .bg(rgb(t.panel))
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.faint))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child("FILE TYPES"),
            )
            .child(div().id("types").flex_1().min_h_0().overflow_y_scroll().child(col))
    }

    fn map_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let Some(l) = &self.loaded else { return div().into_any_element() };
        let focus_node = l.hovered.or(l.selected).unwrap_or(l.zoom);
        let info = {
            let tr = &l.tree;
            let n = focus_node as usize;
            let mut s = format!("{}   {}", tr.path(focus_node), fmt::size(tr.size[n]));
            if tr.is_dir(focus_node) {
                s.push_str(&format!(" · {} files", fmt::count(tr.files[n] as u64)));
            }
            if focus_node != Tree::ROOT {
                s.push_str(&format!(" · {} of parent", fmt::percent(tr.percent_of_parent(focus_node))));
            }
            s
        };
        let paint = Paint { tree: l.tree.clone(), theme: t, hovered: l.hovered, selected: l.selected };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(MAP_BAR_H))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(t.muted))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child(div().flex_1().min_w_0().truncate().child(info))
                    .child(div().flex_none().text_color(rgb(t.faint)).child("Double-click to zoom · Backspace to go up")),
            )
            .child(
                div()
                    .id("map")
                    .flex_1()
                    .min_h_0()
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if this.dragging_split {
                            return;
                        }
                        let hit = this.map.borrow().hit(ev.position);
                        if let Some(l) = this.loaded.as_mut()
                            && l.hovered != hit
                        {
                            l.hovered = hit;
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                        if !*hovering && let Some(l) = this.loaded.as_mut() {
                            l.hovered = None;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.menu = None;
                        let Some(n) = this.map.borrow().hit(ev.position) else { return };
                        if ev.click_count >= 2 {
                            this.zoom_to(n, cx);
                        } else {
                            this.select(n, true, cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        let Some(n) = this.map.borrow().hit(ev.position) else { return };
                        this.select(n, true, cx);
                        this.menu = Some(Menu { at: ev.position, node: n });
                        cx.notify();
                    }))
                    .child(treemap_view::element(self.map.clone(), paint)),
            )
            .into_any_element()
    }

    fn context_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.theme;
        let m = self.menu.as_ref()?;
        let l = self.loaded.as_ref()?;
        let n = m.node;
        let is_dir = l.tree.is_dir(n);
        let deletable = n != Tree::ROOT && !l.tree.has(n, flag::SYNTHETIC);
        let item = |id: &'static str, icon: &'static str, label: &'static str, color: u32| {
            div()
                .id(id)
                .h(px(28.))
                .px_2()
                .mx_1()
                .rounded(px(4.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|s| s.bg(rgba(t.hover)))
                .child(svg().path(icons::path(icon)).size(px(15.)).text_color(rgb(color)))
                .child(label)
        };
        let menu = div()
            .w(px(220.))
            .py_1()
            .flex()
            .flex_col()
            .bg(rgb(t.chrome))
            .border_1()
            .border_color(rgb(t.btn_line))
            .rounded(px(6.))
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .when(is_dir, |d| {
                d.child(item("m-zoom", "zoom-in", "Zoom in here", t.text).on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    this.zoom_to(n, cx);
                })))
            })
            .child(item("m-reveal", "external", "Show in Explorer", t.text).on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.reveal_in_explorer(n, cx);
                cx.notify();
            })))
            .child(item("m-copy", "copy", "Copy path", t.text).on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.copy_path(n, cx);
            })))
            .when(deletable, |d| {
                d.child(div().my_1().h(px(1.)).bg(rgb(t.line))).child(
                    item("m-trash", "trash", "Move to Recycle Bin…", t.danger)
                        .on_click(cx.listener(move |this, _, window, cx| this.recycle(n, window, cx))),
                )
            });
        Some(deferred(anchored().position(m.at).snap_to_window().child(menu)).with_priority(1).into_any_element())
    }

    fn empty_state(&self) -> impl IntoElement + use<> {
        let t = self.theme;
        let line = |k: &'static str, v: &'static str| {
            div().flex().gap_3().child(div().w(px(110.)).text_color(rgb(t.faint)).child(k)).child(div().text_color(rgb(t.muted)).child(v))
        };
        div()
            .size_full()
            .p(px(32.))
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Choose a drive or folder to scan"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .text_size(px(12.))
                    .child(line("Drives", "Pick one on the left."))
                    .child(line("Folder or share", "Ctrl+O, or the button at the top right."))
                    .child(line("In the results", "Arrows move, Enter zooms in, Backspace goes up, Del moves to the Recycle Bin.")),
            )
    }

    fn scanning_state(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let ScanState::Running { root, progress, started } = &self.scan else { return div() };
        let files = progress.files.load(Relaxed);
        let dirs = progress.dirs.load(Relaxed);
        let bytes = progress.bytes.load(Relaxed);
        let secs = started.elapsed().as_secs_f64();
        let phase = match progress.phase() {
            Phase::Starting => "Starting",
            Phase::ReadingMft => "Reading the Master File Table",
            Phase::Walking => "Listing folders",
            Phase::Building => "Building the tree",
        };
        div()
            .size_full()
            .p(px(32.))
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(format!("Scanning {}", root.display())))
            .child(div().text_color(rgb(t.muted)).child(format!(
                "{phase} · {} files · {} folders · {} · {}",
                fmt::count(files),
                fmt::count(dirs),
                fmt::size(bytes),
                fmt::duration(secs)
            )))
            .child(
                div().w(px(420.)).h(px(3.)).rounded(px(2.)).bg(rgb(t.track)).overflow_hidden().child(
                    div()
                        .h_full()
                        .w(px(120.))
                        .rounded(px(2.))
                        .bg(rgb(t.accent))
                        .ml(px((((secs * 260.0) % 540.0) - 120.0) as f32)),
                ),
            )
            .child(div().text_size(px(12.)).text_color(rgb(t.faint)).font_family("Cascadia Mono").truncate().child(progress.current()))
            .child(
                div()
                    .id("cancel")
                    .mt_2()
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .w(px(90.))
                    .justify_center()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(rgb(t.btn_line))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))),
            )
    }

    fn status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let left = if let Some(l) = &self.loaded {
            let tr = &l.tree;
            let mut s = format!(
                "{} files · {} folders · {} · scanned in {} ({})",
                fmt::count(tr.files[0] as u64),
                fmt::count(tr.dirs[0] as u64),
                fmt::size(tr.size[0]),
                fmt::duration(l.elapsed.as_secs_f64()),
                match &l.fallback {
                    Some(why) => format!("{}; MFT not used: {why}", l.backend.label()),
                    None => l.backend.label().to_string(),
                }
            );
            if tr.unreadable > 0 {
                s.push_str(&format!(" · {} folders unreadable", fmt::count(tr.unreadable as u64)));
            }
            s
        } else {
            String::new()
        };
        let free = self.loaded.as_ref().and_then(|l| l.space).map(|(total, free)| format!("{} free of {}", fmt::size(free), fmt::size(total)));
        let (update_text, update_action, emphasized): (String, Option<u8>, bool) = match &self.update {
            UpdateState::Idle => (format!("Diskie {}", env!("CARGO_PKG_VERSION")), Some(0), false),
            UpdateState::Checking { manual: true } => ("Checking for updates…".into(), None, false),
            UpdateState::Checking { manual: false } => (format!("Diskie {}", env!("CARGO_PKG_VERSION")), None, false),
            UpdateState::UpToDate => (format!("Diskie {} is up to date", env!("CARGO_PKG_VERSION")), Some(0), false),
            UpdateState::Available(u) => (format!("Diskie {} is available · Update", u.version), Some(1), true),
            UpdateState::Downloading { version, done, total } => {
                (format!("Downloading {version}… {}%", if *total > 0 { done * 100 / total } else { 0 }), None, false)
            }
            UpdateState::Installed { version, .. } => (format!("{version} installed · Restart"), Some(2), true),
            UpdateState::Failed(e) => (format!("Update failed: {e}"), Some(0), false),
        };
        let update_tip: Option<SharedString> = match (&self.update, update_action) {
            (UpdateState::Failed(e), _) => Some(format!("{e}. Click to try again.").into()),
            (_, Some(0)) => Some("Check for updates".into()),
            (UpdateState::Available(_), _) => Some("Download, verify and install the update".into()),
            (_, Some(2)) => Some("Restart Diskie to finish updating".into()),
            _ => None,
        };
        div()
            .h(px(STATUS_H))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(12.))
            .text_color(rgb(t.muted))
            .bg(rgb(t.chrome))
            .border_t_1()
            .border_color(rgb(t.chrome_line))
            .child(div().flex_1().min_w_0().truncate().child(left))
            .when_some(free, |d, f| d.child(div().flex_none().child(f)))
            .child(
                div()
                    .id("update")
                    .flex_none()
                    .max_w(px(420.))
                    .truncate()
                    .px_1p5()
                    .rounded(px(4.))
                    .when(emphasized, |d| d.text_color(rgb(t.accent)).font_weight(FontWeight::SEMIBOLD))
                    .when(update_action.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(rgba(t.hover))))
                    .child(update_text)
                    .when_some(update_tip, |d, tip| d.tooltip(move |_, cx| cx.new(|_| Tip { text: tip.clone(), theme: t }).into()))
                    .on_click(cx.listener(move |this, _, _, cx| match update_action {
                        Some(0) => this.check_for_updates(true, cx),
                        Some(1) => {
                            if let UpdateState::Available(u) = this.update.clone() {
                                this.install_update(u, cx);
                            }
                        }
                        Some(2) => this.restart(cx),
                        _ => {}
                    })),
            )
    }
}

/// A plain text tooltip in the app's own colours.
struct Tip {
    text: SharedString,
    theme: Theme,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(t.btn_line))
            .bg(rgb(t.chrome))
            .text_color(rgb(t.text))
            .text_size(px(12.))
            .shadow_md()
            .child(self.text.clone())
    }
}

impl Render for Diskie {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_map(cx);
        let t = self.theme;
        let viewport_h = window.viewport_size().height.as_f32();
        let content_h = (viewport_h - TOOLBAR_H - STATUS_H).max(100.0);
        let map_h = (content_h * self.settings.treemap_share.clamp(0.15, 0.85)).round();

        let main: AnyElement = match (&self.scan, &self.loaded) {
            (ScanState::Running { .. }, _) => self.scanning_state(cx).into_any_element(),
            (ScanState::Failed { root, error }, _) => div()
                .size_full()
                .p(px(32.))
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(format!("Couldn't scan {}", root.display())))
                .child(div().text_color(rgb(t.muted)).child(error.clone()))
                .into_any_element(),
            (_, Some(_)) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().flex().child(self.table(cx)).child(self.types_panel()))
                .child(
                    div()
                        .id("split")
                        .h(px(5.))
                        .flex_none()
                        .cursor_row_resize()
                        .border_t_1()
                        .border_color(rgb(t.line))
                        .bg(rgb(if self.dragging_split { t.accent } else { t.canvas }))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.dragging_split = true;
                            cx.notify();
                        })),
                )
                .child(div().h(px(map_h)).flex_none().child(self.map_pane(cx)))
                .into_any_element(),
            _ => self.empty_state().into_any_element(),
        };

        let notice = self.notice.clone().map(|(text, error)| {
            div()
                .h(px(30.))
                .flex_none()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .bg(rgb(t.panel))
                .border_b_1()
                .border_color(rgb(t.line))
                .text_size(px(12.))
                .text_color(rgb(if error { t.danger } else { t.muted }))
                .child(div().flex_1().min_w_0().truncate().child(text))
                .child(
                    div()
                        .id("dismiss")
                        .cursor_pointer()
                        .child(self.icon("x", t.muted).size(px(14.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
        });

        let menu = self.context_menu(cx);

        div()
            .id("root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.canvas))
            .text_color(rgb(t.text))
            .font_family("Segoe UI")
            .text_size(px(13.))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, window, cx| {
                if this.dragging_split {
                    let vh = window.viewport_size().height.as_f32();
                    let content = (vh - TOOLBAR_H - STATUS_H).max(100.0);
                    let y = ev.position.y.as_f32() - TOOLBAR_H;
                    this.settings.treemap_share = ((content - y) / content).clamp(0.15, 0.85);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.dragging_split {
                    this.dragging_split = false;
                    this.settings.save();
                    cx.notify();
                }
            }))
            .child(self.toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(div().flex_1().min_w_0().h_full().flex().flex_col().children(notice).child(div().flex_1().min_h_0().child(main))),
            )
            .child(self.status_bar(cx))
            .children(menu)
    }
}

/// Follows the system theme; `DISKIE_THEME=light` or `dark` overrides it.
fn is_dark(window: &Window) -> bool {
    match std::env::var("DISKIE_THEME").as_deref() {
        Ok("light") => false,
        Ok("dark") => true,
        _ => matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
    }
}

fn num(s: String, w: f32) -> Div {
    div().w(px(w)).flex_none().flex().justify_end().child(s)
}

fn tag(s: &'static str, t: Theme) -> Div {
    div().flex_none().px_1().rounded(px(3.)).border_1().border_color(rgb(t.btn_line)).text_size(px(10.)).text_color(rgb(t.muted)).child(s)
}

fn is_system_path(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_ascii_lowercase();
    [win.as_str(), r"c:\program files", r"c:\program files (x86)", r"c:\programdata"]
        .iter()
        .any(|s| lower == *s || lower.starts_with(&format!("{s}\\")))
        || lower.ends_with(r"\pagefile.sys")
        || lower.ends_with(r"\hiberfil.sys")
        || lower.ends_with(r"\swapfile.sys")
}
