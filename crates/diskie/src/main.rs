//! Diskie: a fast disk space analyzer.
//!
//! `Diskie.exe [folder]` opens the window and, when a folder or drive is given, scans it.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod crash;
mod elevate;
mod icons;
mod settings;
mod theme;
mod treemap_view;

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;

fn main() {
    crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if settings::Settings::load().run_as_admin && elevate::relaunch_elevated(&args) {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut notice = None;
    match diskie_update::on_startup(version) {
        diskie_update::Startup::RolledBack { failed, exe } => {
            // The previous version is back in place; start it and step aside.
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("--rolled-back").arg(&failed);
            if !diskie_scan::volumes::is_elevated() {
                cmd.arg(elevate::NO_ELEVATE_ARG);
            }
            if cmd.spawn().is_ok() {
                return;
            }
        }
        diskie_update::Startup::Updated { from } => notice = Some(format!("Updated from {from} to {version}.")),
        diskie_update::Startup::Normal => {}
    }

    let mut start = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rolled-back" => {
                let failed = it.next().cloned().unwrap_or_default();
                notice = Some(format!("Diskie {failed} didn't start properly, so the previous version was restored."));
            }
            a if a.starts_with("--") => {}
            a => start = Some(PathBuf::from(a)),
        }
    }

    gpui_kit::application().with_assets(icons::Icons).run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(820.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("Diskie".into()), ..Default::default() }),
            window_min_size: Some(size(px(860.), px(520.))),
            app_id: Some("Diskie".into()),
            ..Default::default()
        };
        let start = start.clone();
        let notice = notice.clone();
        gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| app::Diskie::new(start, notice, window, cx))).expect("failed to open the window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();

        // After an update, once this version has run normally for a while, forget the old copy.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(15)).await;
            diskie_update::confirm_healthy();
        })
        .detach();
    });
}
