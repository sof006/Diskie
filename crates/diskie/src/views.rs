//! The flat list views next to the tree: largest files, search results, duplicates, clean-up
//! suggestions and changes since an earlier scan. Each covers the folder the treemap is zoomed
//! into, and is worked out off the UI thread whenever that folder, the tree or the query changes.

use std::collections::HashMap;

use diskie_analyze::cleanup::{self, Safety};
use diskie_analyze::{diff, dupes, search, top};

use super::*;

/// Most rows a list view shows.
const LIMIT: usize = 5_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    Tree,
    Largest,
    Search,
    Duplicates,
    Cleanup,
    Changes,
}

impl View {
    pub const ALL: [View; 6] = [View::Tree, View::Largest, View::Search, View::Duplicates, View::Cleanup, View::Changes];

    pub(super) fn label(self) -> &'static str {
        match self {
            View::Tree => "Tree",
            View::Largest => "Largest files",
            View::Search => "Search",
            View::Duplicates => "Duplicates",
            View::Cleanup => "Clean-up",
            View::Changes => "Changes",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            View::Tree => "list-tree",
            View::Largest => "sort-desc",
            View::Search => "search",
            View::Duplicates => "copy",
            View::Cleanup => "sparkles",
            View::Changes => "history",
        }
    }
}

#[derive(Clone)]
pub(super) enum Row {
    Node(u32),
    /// A line between groups of duplicates.
    Heading(SharedString),
    /// Gone since the compared scan: an index into the diff's `removed`.
    Removed(usize),
}

/// What a list was computed for. A different key means it is out of date.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct ListKey {
    view: View,
    scan: u64,
    zoom: u32,
    generation: u64,
    query: String,
    diff: u64,
}

pub(super) struct ListData {
    pub key: ListKey,
    pub rows: Vec<Row>,
    /// The note column per node (clean-up advice).
    pub notes: HashMap<u32, cleanup::Suggestion>,
    pub summary: String,
    /// Something to show instead of rows (a bad query, nothing to compare with).
    pub message: Option<String>,
}

/// The duplicate finder: it reads files, so it runs only when asked.
#[derive(Default)]
pub(super) struct Dupes {
    pub running: Option<(Arc<dupes::Progress>, Instant)>,
    pub groups: Option<Arc<Vec<dupes::Group>>>,
    /// The folder the results are for.
    pub scope: u32,
    pub min_size: u64,
}

pub(super) const DUPE_SIZES: [(u64, &str); 4] = [(64 << 10, "64 KB"), (1 << 20, "1 MB"), (10 << 20, "10 MB"), (100 << 20, "100 MB")];

impl Diskie {
    fn list_key(&self) -> Option<ListKey> {
        let l = self.loaded.as_ref()?;
        Some(ListKey {
            view: self.view,
            scan: l.scan,
            zoom: l.zoom,
            generation: l.generation,
            query: if self.view == View::Search { self.query.trim().to_string() } else { String::new() },
            diff: l.diff_id,
        })
    }

    /// Makes sure the current list view's rows are up to date, starting the work if needed.
    pub(super) fn ensure_list(&mut self, cx: &mut Context<Self>) {
        if self.view == View::Tree {
            return;
        }
        let Some(key) = self.list_key() else { return };
        let Some(l) = self.loaded.as_mut() else { return };
        if l.list.as_ref().is_some_and(|d| d.key == key) || l.list_pending.as_ref() == Some(&key) {
            return;
        }
        if self.view == View::Duplicates {
            // Built from the finder's results right here: they are already worked out.
            l.list = Some(dupe_rows(l, key));
            return;
        }
        l.list_pending = Some(key.clone());
        let tree = l.tree.clone();
        let diff = l.diff.clone();
        let removed_scope = l.tree.path(l.zoom);
        let now = settings::now();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(old) = self.list_stop.replace(stop.clone()) {
            old.store(true, Relaxed);
        }
        let k2 = key.clone();
        let job = cx.background_spawn(async move { crate::crash::guard(|| compute(&tree, diff.as_deref(), &k2, &removed_scope, now, &stop)) });
        self.list_job = Some(cx.spawn(async move |this, cx| {
            let data = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut() else { return };
                if l.list_pending.as_ref() == Some(&key) {
                    l.list_pending = None;
                }
                match data {
                    Ok(Some(d)) if d.key.scan == l.scan => {
                        l.list = Some(d);
                        cx.notify();
                    }
                    Ok(_) => {}
                    Err(e) => this.notice = Some((format!("Couldn't build the list. {e}"), true)),
                }
            });
        }));
    }

    pub(super) fn set_view(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        self.view = v;
        self.menu = None;
        if v == View::Search {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
        self.flat.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// The nodes of the visible list, in order, for Shift-click ranges and arrow keys.
    pub(super) fn visible_nodes(&self) -> Vec<u32> {
        let Some(l) = &self.loaded else { return Vec::new() };
        if self.view == View::Tree {
            return l.rows.clone();
        }
        l.list
            .as_ref()
            .map(|d| d.rows.iter().filter_map(|r| if let Row::Node(n) = r { Some(*n) } else { None }).filter(|&n| !is_gone(&l.tree, n)).collect())
            .unwrap_or_default()
    }

    pub(super) fn find_duplicates(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        if l.dupes.running.is_some() {
            return;
        }
        let progress = Arc::new(dupes::Progress::default());
        l.dupes.running = Some((progress.clone(), Instant::now()));
        l.dupes.scope = l.zoom;
        let (tree, under, min) = (l.tree.clone(), l.zoom, l.dupes.min_size.max(1));
        let threads = if volumes::has_seek_penalty(&l.root) == Some(true) { 1 } else { 4 };
        let scan = l.scan;
        let p2 = progress.clone();
        let job = cx.background_spawn(async move { crate::crash::guard(|| dupes::find(&tree, under, min, threads, &p2)) });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let busy = this.update(cx, |this, cx| {
                    cx.notify();
                    this.loaded.as_ref().is_some_and(|l| l.dupes.running.is_some())
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(l) = this.loaded.as_mut().filter(|l| l.scan == scan) else { return };
                l.dupes.running = None;
                match r {
                    Ok(Some(groups)) => {
                        let unreadable = progress.unreadable.load(Relaxed);
                        if unreadable > 0 {
                            this.notice = Some((format!("{} files couldn't be read (in use or no access) and were skipped.", fmt::count(unreadable)), false));
                        }
                        l.dupes.groups = Some(Arc::new(groups));
                        l.list = None;
                    }
                    Ok(None) => {}
                    Err(e) => this.notice = Some((format!("Couldn't look for duplicates. {e}"), true)),
                }
                cx.notify();
            });
        });
        self._tasks.push(ticker);
        self._tasks.push(task);
        cx.notify();
    }

    /// Marks every copy but the first in each group, ready to delete.
    pub(super) fn mark_extra_copies(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.loaded.as_mut() else { return };
        let Some(groups) = l.dupes.groups.clone() else { return };
        l.marked.clear();
        for g in groups.iter() {
            let alive: Vec<u32> = g.nodes.iter().copied().filter(|&n| !is_gone(&l.tree, n)).collect();
            l.marked.extend(alive.iter().skip(1));
        }
        cx.notify();
    }

    // ------------------------------------------------------------------------------------
    // Rendering

    pub(super) fn tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let mut row = div().h(px(32.)).flex_none().px_2().flex().items_center().gap_1().border_b_1().border_color(rgb(t.line)).bg(rgb(t.chrome));
        for v in View::ALL {
            let active = self.view == v;
            row = row.child(
                div()
                    .id(("tab", v as usize))
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(rgb(if active { t.text } else { t.muted }))
                    .when(active, |d| d.bg(rgba(t.side_active)).font_weight(FontWeight::SEMIBOLD))
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child(self.icon(v.icon(), if active { t.accent } else { t.faint }).size(px(14.)))
                    .child(v.label())
                    .on_click(cx.listener(move |this, _, window, cx| this.set_view(v, window, cx))),
            );
        }
        let summary = match (&self.loaded, self.view) {
            (Some(l), View::Tree) => l.tree.path(l.zoom),
            (Some(l), _) => l.list.as_ref().filter(|d| Some(&d.key) == self.list_key().as_ref()).map(|d| d.summary.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        row.child(div().flex_1().min_w_0().pl_2().truncate().text_size(px(11.)).text_color(rgb(t.faint)).flex().justify_end().child(summary)).child(
            div()
                .id("owner-toggle")
                .flex_none()
                .h(px(22.))
                .px_1p5()
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(rgb(if self.settings.show_owner { t.text } else { t.faint }))
                .when(self.settings.show_owner, |d| d.bg(rgba(t.side_active)))
                .hover(|s| s.bg(rgba(t.hover)))
                .child(self.icon("user", if self.settings.show_owner { t.accent } else { t.faint }).size(px(13.)))
                .child("Owner")
                .tooltip(move |_, cx| cx.new(|_| Tip { text: "Show who owns each file".into(), theme: t }).into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings.show_owner = !this.settings.show_owner;
                    this.settings.save();
                    cx.notify();
                })),
        )
    }

    /// The column headings and rows of a list view.
    pub(super) fn flat_table(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        self.ensure_list(cx);
        let controls = if self.view == View::Duplicates { Some(self.dupe_controls(cx).into_any_element()) } else { None };
        let Some(l) = &self.loaded else { return div().into_any_element() };
        let show_owner = self.settings.show_owner;
        let note_head = match self.view {
            View::Cleanup => "WHAT IT IS",
            View::Changes => "CHANGE",
            View::Duplicates => "",
            _ => "TYPE",
        };
        let head = |s: &'static str, w: f32| div().w(px(w)).flex_none().flex().justify_end().child(s);
        let header = div()
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
            .child(div().flex_1().min_w_0().pl(px(20.)).child("NAME"))
            .child(div().flex_1().min_w_0().child("IN FOLDER"))
            .child(div().w(px(170.)).flex_none().child(note_head))
            .child(head("SIZE", 76.))
            .child(head("MODIFIED", 64.))
            .when(show_owner, |d| d.child(div().w(px(130.)).flex_none().child("OWNER")));

        let pending = l.list.as_ref().is_none_or(|d| Some(&d.key) != self.list_key().as_ref());
        let (count, message) = match &l.list {
            Some(d) => (d.rows.len(), d.message.clone()),
            None => (0, None),
        };
        let body: AnyElement = if let Some(m) = message.or_else(|| self.empty_message(count, pending)) {
            div().p(px(24.)).text_color(rgb(t.muted)).child(m).into_any_element()
        } else {
            uniform_list("flat", count, cx.processor(|this: &mut Self, range, _, cx| this.flat_rows(range, cx)))
                .track_scroll(&self.flat)
                .flex_1()
                .min_h_0()
                .into_any_element()
        };
        div().flex_1().min_h_0().flex().flex_col().children(controls).child(header).child(body).into_any_element()
    }

    fn empty_message(&self, count: usize, pending: bool) -> Option<String> {
        if count > 0 {
            return None;
        }
        let l = self.loaded.as_ref()?;
        Some(match self.view {
            View::Search if self.query.trim().is_empty() => "Type in the search box above. Besides names you can use >1GB, older:2y, type:video, *.iso, ext:log, is:folder. Every word must match.".into(),
            View::Duplicates if l.dupes.groups.is_none() && l.dupes.running.is_none() => format!(
                "Diskie compares files of the same size, then their contents, below {}. Files that are only in the cloud are never downloaded for this.",
                l.tree.path(l.zoom)
            ),
            View::Duplicates if l.dupes.running.is_some() => String::new(),
            View::Changes if l.diff.is_none() => {
                "No earlier scan of this place to compare with yet. Diskie saves each scan, so scan it again later (or press F5 after you've made changes) to see what grew, shrank, appeared or disappeared.".into()
            }
            _ if pending => "Working…".into(),
            View::Search => "Nothing matches.".into(),
            View::Largest => "No files here.".into(),
            View::Duplicates => "No duplicates found.".into(),
            View::Cleanup => "Nothing Diskie recognises here. It looks for build output, caches, old downloads, crash dumps and similar.".into(),
            View::Changes => "Nothing changed since the compared scan.".into(),
            View::Tree => return None,
        })
    }

    fn dupe_controls(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let Some(l) = &self.loaded else { return div() };
        let running = l.dupes.running.clone();
        let min = l.dupes.min_size;
        let mut sizes = div().flex().items_center().gap_1().child(div().text_color(rgb(t.muted)).child("Files of at least"));
        for (bytes, label) in DUPE_SIZES {
            let on = min == bytes;
            sizes = sizes.child(
                div()
                    .id(("dupe-min", bytes as usize))
                    .px_1p5()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(rgb(if on { t.text } else { t.muted }))
                    .when(on, |d| d.bg(rgba(t.side_active)))
                    .hover(|s| s.bg(rgba(t.hover)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(l) = this.loaded.as_mut() {
                            l.dupes.min_size = bytes;
                            cx.notify();
                        }
                    })),
            );
        }
        let status = match &running {
            Some((p, started)) => {
                let stage = match p.stage.load(Relaxed) {
                    0 => "Grouping by size".to_string(),
                    1 => format!("Comparing the start and end of {} of {} files", fmt::count(p.hashed.load(Relaxed)), fmt::count(p.to_hash.load(Relaxed))),
                    _ => format!("Comparing whole files: {} of {}", fmt::count(p.hashed.load(Relaxed)), fmt::count(p.to_hash.load(Relaxed))),
                };
                format!("{stage} · {} read · {}", fmt::size(p.bytes_read.load(Relaxed)), fmt::duration(started.elapsed().as_secs_f64()))
            }
            None => match &l.list {
                Some(d) if l.dupes.groups.is_some() && d.key.view == View::Duplicates => format!("{} · in {}", d.summary, l.tree.name(l.dupes.scope)),
                _ => String::new(),
            },
        };
        let has_results = l.dupes.groups.as_ref().is_some_and(|g| !g.is_empty());
        div()
            .h(px(34.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(12.))
            .border_b_1()
            .border_color(rgb(t.line))
            .child(match running {
                Some((p, _)) => self.small_button("dupe-cancel", "Cancel").on_click(move |_, _, _| p.cancel()).into_any_element(),
                None => self
                    .small_button("dupe-run", if l.dupes.groups.is_some() { "Search again" } else { "Find duplicates" })
                    .on_click(cx.listener(|this, _, _, cx| this.find_duplicates(cx)))
                    .into_any_element(),
            })
            .child(sizes)
            .child(div().flex_1().min_w_0().truncate().text_color(rgb(t.muted)).child(status))
            .when(has_results, |d| {
                d.child(
                    self.small_button("dupe-mark", "Select extra copies")
                        .tooltip(move |_, cx| cx.new(|_| Tip { text: "Select every copy but the first in each group, ready to delete".into(), theme: t }).into())
                        .on_click(cx.listener(|this, _, _, cx| this.mark_extra_copies(cx))),
                )
            })
    }

    pub(super) fn small_button(&self, id: &'static str, label: &'static str) -> Stateful<Div> {
        let t = self.theme;
        div()
            .id(id)
            .flex_none()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(t.btn_line))
            .text_color(rgb(t.text))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(t.hover)))
            .child(label)
    }

    fn flat_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let t = self.theme;
        let show_owner = self.settings.show_owner;
        let view = self.view;
        let mut want_owner = Vec::new();
        let Some(l) = &self.loaded else { return Vec::new() };
        let Some(data) = &l.list else { return Vec::new() };
        let tree = &l.tree;
        let now = settings::now();
        let root = tree.path(Tree::ROOT);
        let relative = |p: String| -> String {
            let r = p.strip_prefix(&root).unwrap_or(&p).trim_start_matches(['\\', '/']).to_string();
            if r.is_empty() { root.clone() } else { r }
        };
        let colors = crate::colors::change_colors(&t);
        let rows: Vec<Stateful<Div>> = range
            .filter_map(|i| data.rows.get(i).map(|r| (i, r)))
            .map(|(i, row)| {
                let base = div().id(("flat", i)).w_full().h(px(ROW_H)).flex().items_center().px_3().gap_3();
                match row {
                    Row::Heading(text) => base
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.faint))
                        .border_t_1()
                        .border_color(rgb(t.line))
                        .child(text.clone()),
                    Row::Removed(k) => {
                        let Some(r) = l.diff.as_ref().and_then(|d| d.removed.get(*k)) else { return base };
                        let p = std::path::Path::new(&r.path);
                        base.text_color(rgb(t.faint))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .pl(px(20.))
                                    .child(self.icon(if r.is_dir { "folder" } else { "file" }, t.faint).size(px(14.)))
                                    .child(div().min_w_0().truncate().line_through().child(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default())),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(relative(p.parent().map(|x| x.to_string_lossy().to_string()).unwrap_or_default())))
                            .child(div().w(px(170.)).flex_none().text_color(rgb(colors.shrank)).child(format!("Removed · −{}", fmt::size(r.size))))
                            .child(num(String::new(), 76.))
                            .child(num(fmt::age(r.mtime, now), 64.))
                            .when(show_owner, |d| d.child(div().w(px(130.)).flex_none()))
                    }
                    Row::Node(n) => {
                        let n = *n;
                        let ni = n as usize;
                        let gone = is_gone(tree, n);
                        let is_dir = tree.is_dir(n);
                        let selected = l.selected == Some(n) || l.marked.contains(&n);
                        let parent = tree.parent[ni];
                        let folder = if parent == NONE { String::new() } else { relative(tree.path(parent)) };
                        let (note, note_color): (String, u32) = match view {
                            View::Cleanup => match data.notes.get(&n) {
                                Some(s) => (s.what.to_string(), match s.safety {
                                    Safety::Safe => colors.shrank,
                                    Safety::Review => t.text,
                                    Safety::UseTool => t.muted,
                                }),
                                None => (String::new(), t.muted),
                            },
                            View::Changes => match &l.diff {
                                Some(d) if d.is_new(n) => ("New".into(), colors.new),
                                Some(d) => {
                                    let c = d.change(tree, n);
                                    (signed_size(c), if c > 0 { colors.grew } else { colors.shrank })
                                }
                                None => (String::new(), t.muted),
                            },
                            View::Duplicates => (String::new(), t.muted),
                            _ => (Kind::from_u8(tree.kind[ni]).label().to_string(), t.muted),
                        };
                        let size = if tree.has(n, flag::CLOUD) { format!("{} ☁", fmt::size(tree.online_size(n))) } else { fmt::size(tree.size[ni]) };
                        let owner = if show_owner {
                            match l.owners.get(&n) {
                                Some(Some(o)) => o.to_string(),
                                Some(None) => String::new(),
                                None => {
                                    want_owner.push(n);
                                    "…".into()
                                }
                            }
                        } else {
                            String::new()
                        };
                        let tip: Option<SharedString> = data.notes.get(&n).map(|s| format!("{} · {}\n{}", s.what, s.safety.label(), s.advice).into());
                        base.when(selected, |d| d.bg(rgba(t.selection)))
                            .when(!selected, |d| d.hover(|s| s.bg(rgba(t.hover))))
                            .when(gone, |d| d.opacity(0.45))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .pl(px(20.))
                                    .child(self.icon(if is_dir { "folder" } else { "file" }, if is_dir { t.kind(tree.kind[ni]) } else { t.faint }).size(px(14.)))
                                    .child(div().min_w_0().truncate().when(gone, |d| d.line_through()).child(tree.name(n).to_string()))
                                    .when(tree.has(n, flag::CLOUD), |d| d.child(tag("online only", t)))
                                    .when(tree.has(n, flag::REPARSE), |d| d.child(tag("link", t))),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_color(rgb(t.muted)).child(folder))
                            .child(div().w(px(170.)).flex_none().truncate().text_color(rgb(note_color)).child(note))
                            .child(num(size, 76.).text_color(rgb(t.text)))
                            .child(num(fmt::age(tree.mtime[ni], now), 64.).text_color(rgb(t.muted)))
                            .when(show_owner, |d| d.child(div().w(px(130.)).flex_none().truncate().text_color(rgb(t.muted)).child(owner)))
                            .when_some(tip, |d, tip| d.tooltip(move |_, cx| cx.new(|_| Tip { text: tip.clone(), theme: t }).into()))
                            .when(!gone, |d| {
                                d.on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.row_down(n, ev, window, cx)))
                                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(n, ev, cx)))
                            })
                    }
                }
            })
            .collect();
        if !want_owner.is_empty() {
            self.request_owners(want_owner, cx);
        }
        rows
    }
}

/// `1 file`, `3 files`.
pub(super) fn plural(n: u64, one: &str, more: &str) -> String {
    format!("{} {}", fmt::count(n), if n == 1 { one } else { more })
}

/// `+1.2 GB`, `−300 MB`, or nothing for no change.
pub(super) fn signed_size(d: i64) -> String {
    match d {
        0 => String::new(),
        d if d > 0 => format!("+{}", fmt::size(d as u64)),
        d => format!("−{}", fmt::size(d.unsigned_abs())),
    }
}

fn dupe_rows(l: &Loaded, key: ListKey) -> ListData {
    let mut rows = Vec::new();
    let mut summary = String::new();
    if let Some(groups) = &l.dupes.groups {
        let (mut shown, mut wasted) = (0usize, 0u64);
        for g in groups.iter() {
            let alive: Vec<u32> = g.nodes.iter().copied().filter(|&n| !is_gone(&l.tree, n)).collect();
            if alive.len() < 2 {
                continue;
            }
            if rows.len() >= LIMIT {
                break;
            }
            let w = g.size * (alive.len() as u64 - 1);
            wasted += w;
            shown += 1;
            rows.push(Row::Heading(format!("{} copies of {} · {} could be freed", alive.len(), fmt::size(g.size), fmt::size(w)).into()));
            // Deleted copies stay listed, struck through, so the group still makes sense.
            rows.extend(g.nodes.iter().map(|&n| Row::Node(n)));
        }
        summary = format!("{} · {} could be freed", plural(shown as u64, "group", "groups"), fmt::size(wasted));
    }
    ListData { key, rows, notes: HashMap::new(), summary, message: None }
}

/// Works out a list view's rows. `None` when stopped because a newer request replaced it.
fn compute(tree: &Tree, diff: Option<&diff::Diff>, key: &ListKey, scope: &str, now: u64, stop: &std::sync::atomic::AtomicBool) -> Option<ListData> {
    let under = key.zoom;
    let mut notes = HashMap::new();
    let mut message = None;
    let in_scope = if under == Tree::ROOT { String::new() } else { format!(" in {}", tree.name(under)) };
    let rows: Vec<Row>;
    let summary: String;
    match key.view {
        View::Largest => {
            let nodes = top::largest_files(tree, under, LIMIT);
            let bytes: u64 = nodes.iter().map(|&n| tree.size[n as usize]).sum();
            summary = format!("{} largest{in_scope} · {}", plural(nodes.len() as u64, "file", "files"), fmt::size(bytes));
            rows = nodes.into_iter().map(Row::Node).collect();
        }
        View::Search => match search::Query::parse(&key.query) {
            Err(e) => {
                message = Some(e);
                summary = String::new();
                rows = Vec::new();
            }
            Ok(q) if q.is_empty() => {
                summary = String::new();
                rows = Vec::new();
            }
            Ok(q) => {
                let r = q.run(tree, under, now, LIMIT, &|| stop.load(Relaxed))?;
                let shown = if r.total > r.nodes.len() { format!(", largest {} shown", fmt::count(r.nodes.len() as u64)) } else { String::new() };
                summary = format!("{}{in_scope} · {}{shown}", plural(r.total as u64, "match", "matches"), fmt::size(r.bytes));
                rows = r.nodes.into_iter().map(Row::Node).collect();
            }
        },
        View::Cleanup => {
            let found = cleanup::suggest(tree, under, 1 << 20, now);
            let safe: u64 = found.iter().filter(|s| s.safety == Safety::Safe).map(|s| tree.size[s.node as usize]).sum();
            summary = format!("{}{in_scope} · {} safe to delete", plural(found.len() as u64, "suggestion", "suggestions"), fmt::size(safe));
            rows = found.iter().map(|s| Row::Node(s.node)).collect();
            notes = found.into_iter().map(|s| (s.node, s)).collect();
        }
        View::Changes => match diff {
            None => {
                summary = String::new();
                rows = Vec::new();
            }
            Some(d) => {
                let changed = diff::changed(tree, d, under, LIMIT);
                // Removed entries below the scope, merged in by size.
                let sep = if scope.ends_with(['\\', '/']) { String::new() } else { std::path::MAIN_SEPARATOR.to_string() };
                let prefix = format!("{scope}{sep}");
                let removed: Vec<usize> = (0..d.removed.len()).filter(|&i| under == Tree::ROOT || d.removed[i].path.starts_with(&prefix)).take(LIMIT).collect();
                let mut all: Vec<(u64, Row)> = changed.iter().map(|&n| (d.change(tree, n).unsigned_abs(), Row::Node(n))).collect();
                all.extend(removed.iter().map(|&i| (d.removed[i].size, Row::Removed(i))));
                all.sort_by_key(|x| std::cmp::Reverse(x.0));
                all.truncate(LIMIT);
                let net = d.change(tree, under);
                summary = format!("Net change{in_scope}: {}", if net == 0 { "none".to_string() } else { signed_size(net) });
                rows = all.into_iter().map(|(_, r)| r).collect();
            }
        },
        View::Tree | View::Duplicates => return None,
    }
    Some(ListData { key: key.clone(), rows, notes, summary, message })
}
