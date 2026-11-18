//! The window frame Diskie draws itself when the desktop doesn't (GNOME and WSLg on Wayland
//! leave title bars to the app): resize edges around the window and the minimize, maximize and
//! close buttons. Moving the window by its toolbar is wired up in the toolbar itself.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

/// Width of the invisible strips along the window's edges that resize it.
const EDGE: f32 = 5.0;

/// Whether this window has to draw its own frame, and if so which edges are free to resize
/// (an edge snapped to the screen or another window is not).
pub fn client_frame(window: &Window) -> Option<Tiling> {
    match window.window_decorations() {
        Decorations::Client { tiling } => Some(tiling),
        Decorations::Server => None,
    }
}

/// Strips and corners along the free edges, drawn over everything else.
pub fn resize_handles(tiling: Tiling, maximized: bool) -> Vec<AnyElement> {
    if maximized || tiling == Tiling::tiled() {
        return Vec::new();
    }
    let e = px(EDGE);
    let c = px(EDGE * 2.0);
    let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
    let strip = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div()
            .id(id)
            .absolute()
            .occlude()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(edge);
            })
    };
    let mut out = Vec::new();
    if top {
        out.push(strip("edge-t", ResizeEdge::Top, CursorStyle::ResizeUpDown).top_0().left(c).right(c).h(e).into_any_element());
    }
    if bottom {
        out.push(strip("edge-b", ResizeEdge::Bottom, CursorStyle::ResizeUpDown).bottom_0().left(c).right(c).h(e).into_any_element());
    }
    if left {
        out.push(strip("edge-l", ResizeEdge::Left, CursorStyle::ResizeLeftRight).left_0().top(c).bottom(c).w(e).into_any_element());
    }
    if right {
        out.push(strip("edge-r", ResizeEdge::Right, CursorStyle::ResizeLeftRight).right_0().top(c).bottom(c).w(e).into_any_element());
    }
    if top && left {
        out.push(strip("edge-tl", ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight).top_0().left_0().size(c).into_any_element());
    }
    if top && right {
        out.push(strip("edge-tr", ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft).top_0().right_0().size(c).into_any_element());
    }
    if bottom && left {
        out.push(strip("edge-bl", ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().size(c).into_any_element());
    }
    if bottom && right {
        out.push(strip("edge-br", ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().size(c).into_any_element());
    }
    out
}

/// Minimize, maximize (or restore) and close, for the right end of the toolbar.
pub fn controls(t: Theme, maximized: bool, icon: impl Fn(&'static str, u32) -> Svg) -> Div {
    let button = |id: &'static str, glyph: &'static str, danger: bool| {
        div()
            .id(id)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(danger, |d| d.hover(move |s| s.bg(rgb(t.danger)).text_color(rgb(0xffffff))))
            .when(!danger, |d| d.hover(|s| s.bg(rgba(t.hover))))
            .child(icon(glyph, t.muted).size(px(14.)))
            // Keep the press from starting a window move in the toolbar underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_0p5()
        .ml_1()
        .child(button("win-min", "minimize", false).on_click(|_, window, _| window.minimize_window()))
        .child(button("win-max", if maximized { "restore" } else { "maximize" }, false).on_click(|_, window, _| window.zoom_window()))
        .child(button("win-close", "x", true).on_click(|_, window, _| window.remove_window()))
}
