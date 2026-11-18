//! The window's newer features that aren't list views: multi-select, snapshots and the change
//! comparison, live updates, Recycle Bin sizes, cloud files, export, owners, and the colour
//! legend on the right.

use std::collections::HashSet;

use diskie_analyze::{age, diff, export};
use diskie_store::Info;

use super::views::{Row, View, plural, signed_size};
use super::*;
use crate::bins::Bin;
use crate::colors::{self, ColorBy};
use crate::live::{self, Live};

/// How often changes on disk are patched into the tree.
const LIVE_EVERY: Duration = Duration::from_secs(3);

pub(super) enum LiveState {
    Off,
    Starting,
    On(Live),
    Unavailable(String),
}

/// What live updates are doing for the loaded scan, beyond the watcher itself.
#[derive(Default)]
pub(super) struct LiveWork {
    /// Paths taken from the watcher, waiting to be looked at.
    pub backlog: Vec<PathBuf>,
    pub busy: bool,
    /// Changes worked out but not applied yet, because the tree was in use elsewhere.
    pub ready: Option<Vec<live::Change>>,
    pub waits: u32,
}

/// The change comparison's totals at the zoomed folder.
#[derive(Clone, Copy, Default)]
pub(super) struct ChangeTotals {
    pub new_files: u64,
    pub new_bytes: u64,
    pub grew: u64,
    pub shrank: u64,
    pub removed: u64,
    pub net: i64,
}

/// `n` or a folder above it was deleted.
pub(super) fn is_gone(tree: &Tree, mut n: u32) -> bool {
    while n != NONE {
        if tree.has(n, flag::DELETED) {
            return true;
        }
        n = tree.parent[n as usize];
    }
    false
}

pub(super) fn next_diff_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Relaxed)
}

impl Loaded {
    /// After the tree changed under the view (deletes, live updates): keep the view on things
    /// that still exist.
    pub(super) fn tidy(&mut self) {
        self.generation += 1;
        let n = self.tree.len();
        if self.expanded.len() < n {
            self.expanded.resize(n, false);
        }
        while self.zoom != Tree::ROOT && is_gone(&self.tree, self.zoom) {
            self.zoom = self.tree.parent[self.zoom as usize];
        }
        if let Some(mut s) = self.selected
            && is_gone(&self.tree, s)
        {
            while s != Tree::ROOT && is_gone(&self.tree, s) {
                s = self.tree.parent[s as usize];
            }
            self.selected = Some(s);
        }
        let tree = &self.tree;
        self.marked.retain(|&m| !is_gone(tree, m));
        self.owners.retain(|&k, _| (k as usize) < n);
        self.rebuild_rows();
        self.space = volumes::space(&self.root);
    }

    fn age_totals(&mut self, now: u64) -> ([u64; age::BUCKETS], u64) {
        match self.age_totals {
            Some((z, g, h, u)) if z == self.zoom && g == self.generation => (h, u),
            _ => {
                let (h, u) = age::histogram(&self.tree, self.zoom, now);
                self.age_totals = Some((self.zoom, self.generation, h, u));
                (h, u)
            }
        }
    }

    fn change_totals(&mut self) -> Option<ChangeTotals> {
        let d = self.diff.clone()?;
        if let Some((z, g, id, t)) = self.change_totals
            && z == self.zoom
            && g == self.generation
            && id == self.diff_id
        {
            return Some(t);
        }
        let tree = &self.tree;
        let mut t = ChangeTotals { net: d.change(tree, self.zoom), ..Default::default() };
        let mut stack = vec![self.zoom];
        while let Some(n) = stack.pop() {
            if tree.is_dir(n) {
                stack.extend_from_slice(tree.kids(n));
            } else if d.is_new(n) {
                t.new_files += 1;
                t.new_bytes += tree.size[n as usize];
            } else {
                match d.change(tree, n) {
                    c if c > 0 => t.grew += c as u64,
                    c => t.shrank += c.unsigned_abs(),
                }
            }
        }
        let scope = tree.path(self.zoom);
        let sep = if scope.ends_with(['\\', '/']) { String::new() } else { std::path::MAIN_SEPARATOR.to_string() };
        let prefix = format!("{scope}{sep}");
        t.removed = d.removed.iter().filter(|r| self.zoom == Tree::ROOT || r.path.starts_with(&prefix)).map(|r| r.size).sum();
        self.change_totals = Some((self.zoom, self.generation, self.diff_id, t));
        Some(t)
    }
}

impl Diskie {
    // ------------------------------------------------------------------------------------
    // Selection

    /// What an action applies to: the Ctrl/Shift-selected items, or else the selected one.
    pub(super) fn chosen(&self) -> Vec<u32> {
        let Some(l) = &self.loaded else { return Vec::new() };
        if l.marked.is_empty() {
            return l.selected.into_iter().collect();
        }
        let mut v: Vec<u32> = l.marked.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// A press on a row of the tree or of a list: select, Ctrl to add or remove, Shift for a
    /// range, double-click to open.
    pub(super) fn row_down(&mut self, n: u32, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        self.menu = None;
        let ctrl = ev.modifiers.control || ev.modifiers.platform;
        let shift = ev.modifiers.shift;
        if ev.click_count >= 2 && !ctrl && !shift {
            self.open(n, window, cx);
            return;
        }
        let range = if shift { self.visible_nodes() } else { Vec::new() };
        let Some(l) = self.loaded.as_mut() else { return };
        if ctrl {
            if l.marked.is_empty()
                && let Some(s) = l.selected
                && s != Tree::ROOT
            {
                l.marked.insert(s);
            }
            if !l.marked.remove(&n) {
                l.marked.insert(n);
            }
            l.anchor = Some(n);
        } else if shift {
            let from = l.anchor.or(l.selected).and_then(|a| range.iter().position(|&r| r == a));
            let to = range.iter().position(|&r| r == n);
            l.marked.clear();
            match (from, to) {
                (Some(a), Some(b)) => l.marked.extend(range[a.min(b)..=a.max(b)].iter().copied()),
                _ => {
                    l.marked.insert(n);
                }
            }
        } else {
            l.marked.clear();
            l.anchor = Some(n);
        }
        l.selected = Some(n);
        cx.notify();
    }

    /// Double-click: a folder opens in the tree, a file in the file manager, and another drive
    /// mounted inside the scan is scanned on its own.
    pub(super) fn open(&mut self, n: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        if l.tree.has(n, flag::MOUNT) {
            let path = PathBuf::from(l.tree.path(n));
            self.start_scan(path, window, cx);
        } else if l.tree.is_dir(n) {
            self.view = View::Tree;
            self.zoom_to(n, cx);
        } else {
            self.reveal_in_explorer(n, cx);
        }
    }

    /// Switches to the tree with `n` selected and in view.
    pub(super) fn show_in_tree(&mut self, n: u32, cx: &mut Context<Self>) {
        self.view = View::Tree;
        self.menu = None;
        if let Some(l) = self.loaded.as_mut()
            && !l.tree.is_ancestor(l.zoom, n)
        {
            l.zoom = Tree::ROOT;
        }
        self.select(n, true, cx);
    }

    pub(super) fn open_menu(&mut self, n: u32, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if !l.marked.contains(&n) {
            l.marked.clear();
            l.anchor = Some(n);
        }
        l.selected = Some(n);
        let count = if l.marked.is_empty() { 1 } else { l.marked.len() };
        self.menu = Some(Menu::new(ev.position, n, ev.modifiers.shift, &l.tree, count));
        cx.notify();
    }

    // ------------------------------------------------------------------------------------
    // After a scan

    /// Saves the scan as a snapshot and compares it with the previous one, starts watching
    /// for changes, and refreshes the bin sizes.
    pub(super) fn after_load(&mut self, cx: &mut Context<Self>) {
        self.refresh_bins(cx);
        self.start_live(cx);
        let Some(l) = &self.loaded else { return };
        if !self.settings.snapshots {
            return;
        }
        let Some(base) = diskie_store::base_dir() else { return };
        let (tree, scan, taken) = (l.tree.clone(), l.scan, l.taken);
        let job = cx.background_spawn(async move {
            crate::crash::guard(|| {
                let root = tree.name(Tree::ROOT).to_string();
                let previous = diskie_store::list(&base, &root).into_iter().next();
                let compared = previous.and_then(|info| diskie_store::load(&info.path).ok()).map(|(mut info, old)| {
                    let d = diff::compare(&tree, &old);
                    info.path = PathBuf::new();
                    (info, d)
                });
                let saved = diskie_store::save(&base, &root, &tree, taken).err().map(|e| e.to_string());
                (diskie_store::list(&base, &root), compared, saved)
            })
        });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) else { return };
                match r {
                    Ok((list, compared, error)) => {
                        l.snapshots = list;
                        if let Some((info, d)) = compared {
                            l.compared = Some(info);
                            l.diff = Some(Arc::new(d));
                            l.diff_id = next_diff_id();
                        }
                        if let Some(e) = error {
                            this.notice = Some((format!("Couldn't save this scan for comparing later: {e}"), true));
                        }
                    }
                    Err(e) => this.notice = Some((format!("Couldn't compare with the previous scan. {e}"), true)),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Compares the loaded scan with an earlier snapshot of the same place.
    pub(super) fn compare_with(&mut self, info: Info, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        let (tree, scan, path) = (l.tree.clone(), l.scan, info.path.clone());
        let job = cx.background_spawn(async move {
            crate::crash::guard(|| diskie_store::load(&path).map(|(_, old)| diff::compare(&tree, &old)).map_err(|e| e.to_string()))
        });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) else { return };
                match r {
                    Ok(Ok(d)) => {
                        l.diff = Some(Arc::new(d));
                        l.diff_id = next_diff_id();
                        l.compared = Some(info);
                    }
                    Ok(Err(e)) | Err(e) => this.notice = Some((format!("Couldn't read that earlier scan: {e}"), true)),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------------------------
    // Live updates

    pub(super) fn start_live(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if !self.settings.live_updates {
            l.live = LiveState::Off;
            return;
        }
        #[cfg(not(windows))]
        if l.tree.dirs[0] > live::MAX_LINUX_FOLDERS {
            l.live = LiveState::Unavailable(format!(
                "This scan has {} folders; Linux needs a watch for each, so live updates are off past {}.",
                fmt::count(l.tree.dirs[0] as u64),
                fmt::count(live::MAX_LINUX_FOLDERS as u64)
            ));
            return;
        }
        l.live = LiveState::Starting;
        l.live_work = LiveWork::default();
        let (root, scan) = (l.root.clone(), l.scan);
        let job = cx.background_spawn(async move { Live::start(&root) });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) else { return };
                if !matches!(l.live, LiveState::Starting) {
                    return; // turned off meanwhile
                }
                l.live = match r {
                    Ok(w) => LiveState::On(w),
                    Err(e) => LiveState::Unavailable(e),
                };
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    pub(super) fn toggle_live(&mut self, cx: &mut Context<Self>) {
        self.settings.live_updates = !self.settings.live_updates;
        self.settings.save();
        self.start_live(cx);
        cx.notify();
    }

    /// Runs for the life of the window: takes what changed on disk and patches it in.
    pub(super) fn live_loop(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LIVE_EVERY).await;
                if this.update(cx, |this, cx| this.live_tick(cx)).is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    fn live_tick(&mut self, cx: &mut Context<Self>) {
        // Deleting from Diskie patches the tree itself; don't race it.
        if self.deleting.is_some() {
            return;
        }
        let Some(l) = self.loaded.as_mut() else { return };
        let LiveState::On(w) = &l.live else { return };
        let (paths, missed) = w.take();
        if missed {
            self.notice = Some(("Some changes on disk came too fast to follow. Rescan (F5) for exact numbers.".into(), false));
            cx.notify();
        }
        l.live_work.backlog.extend(paths);
        if let Some(changes) = l.live_work.ready.take() {
            self.apply_live(changes, cx);
            return;
        }
        let Some(l) = self.loaded.as_mut() else { return };
        if l.live_work.busy || l.live_work.backlog.is_empty() {
            return;
        }
        l.live_work.busy = true;
        let paths = std::mem::take(&mut l.live_work.backlog);
        let (tree, scan) = (l.tree.clone(), l.scan);
        let job = cx.background_spawn(async move { crate::crash::guard(|| live::resolve(&tree, paths)) });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) else { return };
                l.live_work.busy = false;
                if let Ok(changes) = r
                    && !changes.is_empty()
                {
                    this.apply_live(changes, cx);
                }
            });
        });
        self._tasks.push(task);
    }

    fn apply_live(&mut self, changes: Vec<live::Change>, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        // Changing a tree that a background job still reads would copy all of it; wait a
        // little for the job to finish first.
        if Arc::get_mut(&mut l.tree).is_none() && l.live_work.waits < 3 {
            l.live_work.waits += 1;
            l.live_work.ready = Some(changes);
            return;
        }
        l.live_work.waits = 0;
        let applied = live::apply(Arc::make_mut(&mut l.tree), &changes);
        if applied > 0 {
            l.tidy();
            cx.notify();
        }
    }

    // ------------------------------------------------------------------------------------
    // Recycle Bins

    pub(super) fn refresh_bins(&mut self, cx: &mut Context<Self>) {
        let volumes = self.volumes.clone();
        let job = cx.background_spawn(async move { crate::crash::guard(|| crate::bins::list(&volumes)).unwrap_or_default() });
        let task = cx.spawn(async move |this, cx| {
            let bins = job.await;
            let _ = this.update(cx, |this, cx| {
                this.bins = bins;
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn empty_bin(&mut self, bin: Bin, window: &mut Window, cx: &mut Context<Self>) {
        let title = format!("Empty the {} on {}?", words::BIN, bin.root.trim_end_matches(['\\', '/']));
        let detail = format!(
            "{} in {} items will be deleted for good. This can't be undone.",
            fmt::size(bin.bytes),
            fmt::count(bin.items)
        );
        let answer = window.prompt(PromptLevel::Critical, &title, Some(&detail), &[&format!("Empty {}", words::BIN), "Cancel"], cx);
        let task = cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let b2 = bin.clone();
            let r = cx.background_spawn(async move { crate::bins::empty(&b2) }).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(match r {
                    Ok(()) => (format!("Emptied the {} on {} ({} freed)", words::BIN, bin.root.trim_end_matches(['\\', '/']), fmt::size(bin.bytes)), false),
                    Err(e) => (format!("Couldn't empty the {} on {}: {e}", words::BIN, bin.root), true),
                });
                this.volumes = volumes::list_volumes();
                if let Some(l) = this.loaded.as_mut() {
                    l.space = volumes::space(&l.root);
                }
                this.refresh_bins(cx);
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// The sidebar's list of bins that hold something.
    pub(super) fn bins_section(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        if self.bins.is_empty() {
            return None;
        }
        let t = self.theme;
        let mut col = div().flex().flex_col().child(
            div().px_3().pt_3().pb_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.faint)).child(words::BIN.to_uppercase()),
        );
        for (i, b) in self.bins.clone().into_iter().enumerate() {
            let label = b.root.trim_end_matches(['\\', '/']).to_string();
            let label = if label.is_empty() { "/".to_string() } else { label };
            col = col.child(
                div()
                    .mx_1p5()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.icon("trash", t.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(label))
                            .child(div().text_size(px(11.)).text_color(rgb(t.muted)).child(format!("{} · {} items", fmt::size(b.bytes), fmt::count(b.items)))),
                    )
                    .child(
                        div()
                            .id(("empty-bin", i))
                            .flex_none()
                            .h(px(22.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(rgb(t.btn_line))
                            .text_size(px(11.))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(t.hover)))
                            .child("Empty")
                            .on_click(cx.listener(move |this, _, window, cx| this.empty_bin(b.clone(), window, cx))),
                    ),
            );
        }
        Some(col)
    }

    // ------------------------------------------------------------------------------------
    // Cloud files

    /// Asks OneDrive (or another cloud provider) to keep `n` online only, or with
    /// `keep_local` always on this device.
    pub(super) fn set_cloud(&mut self, n: u32, keep_local: bool, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        self.menu = None;
        let (path, dir, name) = (l.tree.path(n), l.tree.is_dir(n), l.tree.name(n).to_string());
        let job = cx.background_spawn(async move { crate::files::set_cloud(Path::new(&path), dir, keep_local) });
        let live = self.settings.live_updates;
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let later = if live { "Diskie updates as it goes" } else { "rescan (F5) in a moment to see it" };
                this.notice = Some(match r {
                    Ok(()) if keep_local => (format!("{name} will be kept on this device; it downloads in the background."), false),
                    Ok(()) => (format!("Freeing up the space {name} takes; {later}."), false),
                    Err(e) => (format!("Couldn't change {name}: {e}"), true),
                });
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------------------------
    // Export

    /// Saves what the current view shows as CSV, or JSON when the name ends in `.json`.
    pub(super) fn export(&mut self, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        let tree = l.tree.clone();
        let nodes: Option<Vec<u32>> = match self.view {
            View::Tree => None,
            _ => Some(
                l.list
                    .as_ref()
                    .map(|d| d.rows.iter().filter_map(|r| if let Row::Node(n) = r { Some(*n) } else { None }).filter(|&n| !is_gone(&tree, n)).collect())
                    .unwrap_or_default(),
            ),
        };
        let zoom = l.zoom;
        // The folder's own name, not the whole path the scan started from.
        let folder = Path::new(tree.name(zoom)).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| tree.name(zoom).to_string());
        let what = match self.view {
            View::Tree => folder,
            v => format!("{folder} {}", v.label().to_lowercase()),
        };
        let clean: String = what.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let suggested = format!("Diskie {}.csv", clean.trim_matches(['_', ' ']));
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        let dir = if home.join("Downloads").is_dir() { home.join("Downloads") } else { home };
        let rx = cx.prompt_for_new_path(&dir, Some(&suggested));
        let task = cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let p2 = path.clone();
            let r = cx
                .background_spawn(async move {
                    let json = p2.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
                    let file = std::fs::File::create(&p2)?;
                    let mut w = std::io::BufWriter::new(file);
                    let nodes: Box<dyn Iterator<Item = u32>> = match nodes {
                        Some(v) => Box::new(v.into_iter()),
                        None => Box::new(tree.subtree(zoom)),
                    };
                    if json { export::json_list(&tree, nodes, &mut w) } else { export::csv(&tree, nodes, &mut w) }?;
                    std::io::Write::flush(&mut w)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(match r {
                    Ok(()) => (format!("Saved {}", path.display()), false),
                    Err(e) => (format!("Couldn't save {}: {e}", path.display()), true),
                });
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------------------------
    // Owners

    /// Looks up the owners of `nodes` off the UI thread.
    pub(super) fn request_owners(&mut self, nodes: Vec<u32>, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        let pending: Arc<str> = Arc::from("…");
        let mut jobs: Vec<(u32, PathBuf)> = Vec::new();
        for n in nodes {
            if let std::collections::hash_map::Entry::Vacant(e) = l.owners.entry(n) {
                e.insert(Some(pending.clone()));
                jobs.push((n, PathBuf::from(l.tree.path(n))));
            }
        }
        if jobs.is_empty() {
            return;
        }
        let (owners, scan) = (self.owner_names.clone(), l.scan);
        let job = cx.background_spawn(async move { jobs.into_iter().map(|(n, p)| (n, owners.of(&p))).collect::<Vec<_>>() });
        let task = cx.spawn(async move |this, cx| {
            let found = job.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) {
                    l.owners.extend(found);
                    cx.notify();
                }
            });
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------------------------
    // The right-hand panel: what the treemap's colours mean

    pub(super) fn legend_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let by = self.settings.color_by;
        let mut switch = div().flex().items_center().gap_0p5();
        for c in ColorBy::ALL {
            let on = c == by;
            switch = switch.child(
                div()
                    .id(("color-by", c as usize))
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(rgb(if on { t.text } else { t.faint }))
                    .when(on, |d| d.bg(rgba(t.side_active)))
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child(c.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.color_by = c;
                        this.settings.save();
                        cx.notify();
                    })),
            );
        }
        let body: AnyElement = match by {
            ColorBy::Type => self.types_legend().into_any_element(),
            ColorBy::Age => self.age_legend().into_any_element(),
            ColorBy::Change => self.change_legend(cx).into_any_element(),
        };
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
                    .h(px(32.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.faint))
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child("COLOUR BY")
                    .child(switch),
            )
            .child(div().id("legend").flex_1().min_h_0().overflow_y_scroll().child(body))
    }

    fn legend_row(&self, color: u32, label: String, value: String, frac: f32) -> Div {
        let t = self.theme;
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
                    .child(div().size(px(10.)).flex_none().rounded(px(2.)).bg(rgb(color)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().flex_none().child(value)),
            )
            .child(
                div()
                    .ml(px(18.))
                    .h(px(3.))
                    .rounded(px(2.))
                    .bg(rgb(t.track))
                    .child(div().h_full().rounded(px(2.)).w(relative(frac.clamp(0.0, 1.0))).bg(rgb(color))),
            )
    }

    fn types_legend(&mut self) -> Div {
        let t = self.theme;
        let mut col = div().flex().flex_col().gap_0p5().px_3().pt_2();
        let Some(l) = self.loaded.as_mut() else { return col };
        let totals = l.kind_totals();
        let all: u64 = totals.iter().sum::<u64>().max(1);
        let mut order: Vec<usize> = (0..KIND_COUNT).filter(|&k| totals[k] > 0).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(totals[k]));
        for k in order {
            col = col.child(self.legend_row(t.kind(k as u8), Kind::from_u8(k as u8).label().into(), fmt::size(totals[k]), totals[k] as f32 / all as f32));
        }
        col
    }

    fn age_legend(&mut self) -> Div {
        let t = self.theme;
        let mut col = div().flex().flex_col().gap_0p5().px_3().pt_2();
        let now = settings::now();
        let Some(l) = self.loaded.as_mut() else { return col };
        let (h, unknown) = l.age_totals(now);
        let all = (h.iter().sum::<u64>() + unknown).max(1);
        let colors = colors::age_colors();
        col = col.child(div().pb_1().text_size(px(11.)).text_color(rgb(t.muted)).child("By when each file was last changed"));
        for (i, label) in age::LABELS.iter().enumerate() {
            col = col.child(self.legend_row(colors[i], label.to_string(), fmt::size(h[i]), h[i] as f32 / all as f32));
        }
        if unknown > 0 {
            col = col.child(self.legend_row(colors::unknown_color(&t), "Unknown date".into(), fmt::size(unknown), unknown as f32 / all as f32));
        }
        col
    }

    fn change_legend(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let mut col = div().flex().flex_col().gap_1().px_3().pt_2().text_size(px(12.));
        let snapshots_on = self.settings.snapshots;
        let Some(l) = self.loaded.as_mut() else { return col };
        let now = settings::now();
        let c = colors::change_colors(&t);
        match (l.change_totals(), &l.compared) {
            (Some(tot), Some(info)) => {
                col = col
                    .child(div().text_color(rgb(t.muted)).child(format!("Compared with the scan from {} ago ({})", fmt::age(info.taken as u32, now), fmt::size(info.size))))
                    .child(
                        div()
                            .py_1()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if tot.net == 0 { "No change".to_string() } else { signed_size(tot.net) }),
                    );
                let big = [tot.new_bytes, tot.grew, tot.shrank, tot.removed].into_iter().max().unwrap_or(0).max(1) as f32;
                col = col
                    .child(self.legend_row(c.new, format!("New ({})", plural(tot.new_files, "file", "files")), fmt::size(tot.new_bytes), tot.new_bytes as f32 / big))
                    .child(self.legend_row(c.grew, "Grew".into(), format!("+{}", fmt::size(tot.grew)), tot.grew as f32 / big))
                    .child(self.legend_row(c.shrank, "Shrank".into(), format!("−{}", fmt::size(tot.shrank)), tot.shrank as f32 / big))
                    .child(self.legend_row(c.same, "Removed (not on the map)".into(), format!("−{}", fmt::size(tot.removed)), tot.removed as f32 / big));
            }
            _ if !snapshots_on => {
                col = col.child(div().text_color(rgb(t.muted)).child("Saving scans is off, so there is nothing to compare with."));
            }
            _ => {
                col = col.child(div().text_color(rgb(t.muted)).child(
                    "No earlier scan of this place yet. Diskie keeps each scan, so the next scan (F5) shows what grew, shrank, appeared or disappeared.",
                ));
            }
        }
        let Some(l) = self.loaded.as_ref() else { return col };
        let others: Vec<Info> = l.snapshots.iter().filter(|s| s.taken != l.taken).cloned().collect();
        if !others.is_empty() {
            col = col.child(div().pt_3().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(t.faint)).child("COMPARE WITH"));
            for (i, s) in others.into_iter().enumerate() {
                let on = l.compared.as_ref().is_some_and(|c| c.taken == s.taken);
                let text = format!("{} ago · {}", fmt::age(s.taken as u32, now), fmt::size(s.size));
                col = col.child(
                    div()
                        .id(("snapshot", i))
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(rgb(if on { t.text } else { t.muted }))
                        .when(on, |d| d.bg(rgba(t.side_active)))
                        .hover(|s| s.bg(rgba(t.hover)))
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| this.compare_with(s.clone(), cx))),
                );
            }
        }
        col.child(
            div()
                .id("snapshots-toggle")
                .mt_3()
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(rgb(t.faint))
                .hover(|s| s.bg(rgba(t.hover)))
                .child(if snapshots_on { "Keeping the last 5 scans of each place · Turn off" } else { "Keep scans to compare · Turn on" })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.snapshots = !this.settings.snapshots;
                    this.settings.save();
                    cx.notify();
                })),
        )
    }

    /// The status bar's live-updates switch.
    pub(super) fn live_indicator(&mut self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let t = self.theme;
        let l = self.loaded.as_ref()?;
        let (text, tip, color): (&str, SharedString, u32) = match &l.live {
            LiveState::On(_) => ("Live", "Changes on disk show up as they happen. Click to stop.".into(), colors::change_colors(&t).shrank),
            LiveState::Starting => ("Live…", "Starting to watch for changes".into(), t.faint),
            LiveState::Off => ("Live off", "Click to keep the numbers up to date as files change".into(), t.faint),
            LiveState::Unavailable(why) => ("Live unavailable", format!("{why} Click to turn live updates off.").into(), t.danger),
        };
        Some(
            div()
                .id("live")
                .flex_none()
                .px_1p5()
                .rounded(px(4.))
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.hover)))
                .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                .child(text)
                .tooltip(move |_, cx| cx.new(|_| Tip { text: tip.clone(), theme: t }).into())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_live(cx))),
        )
    }
}

/// Marked nodes to draw on the treemap: a few thousand at most.
pub(super) fn marked_for_map(marked: &HashSet<u32>) -> Vec<u32> {
    marked.iter().copied().take(5_000).collect()
}
