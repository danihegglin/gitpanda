//! Client-side window decorations, for compositors that leave them to the app
//! (GNOME on Wayland, as on RHEL 9): window buttons, resize edges and a shadow.
//! Everywhere else the platform draws these and this module draws nothing.

use gpui::{
    AnyElement, BoxShadow, CursorStyle, Decorations, MouseButton, ResizeEdge, Stateful, Div, Window,
    div, point, prelude::*, px,
};

use super::theme::*;

/// Width of the transparent band around the window that resizes it and holds
/// its shadow.
const INSET: f32 = 10.;

fn client_side(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Wraps the window's content in resize edges and a shadow when the app has to
/// draw its own decorations.
pub fn frame(content: AnyElement, window: &mut Window) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return content;
    };
    window.set_client_inset(px(INSET));
    let inset = px(INSET);
    let edge = |id: &'static str, e: ResizeEdge, cursor: CursorStyle| {
        div()
            .id(id)
            .absolute()
            .occlude()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, _| window.start_window_resize(e))
    };
    let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
    div()
        .relative()
        .size_full()
        .when(top, |d| d.pt(inset))
        .when(bottom, |d| d.pb(inset))
        .when(left, |d| d.pl(inset))
        .when(right, |d| d.pr(inset))
        .child(
            div()
                .size_full()
                .border_color(c(BORDER_HI))
                .when(top, |d| d.border_t_1())
                .when(bottom, |d| d.border_b_1())
                .when(left, |d| d.border_l_1())
                .when(right, |d| d.border_r_1())
                .when(!tiling.is_tiled(), |d| {
                    d.shadow(vec![BoxShadow {
                        color: alpha(0, 0.45).into(),
                        blur_radius: inset / 2.,
                        spread_radius: px(0.),
                        offset: point(px(0.), px(0.)),
                    }])
                })
                .child(content),
        )
        .when(top, |d| d.child(edge("resize-t", ResizeEdge::Top, CursorStyle::ResizeUpDown).top_0().left_0().right_0().h(inset)))
        .when(bottom, |d| d.child(edge("resize-b", ResizeEdge::Bottom, CursorStyle::ResizeUpDown).bottom_0().left_0().right_0().h(inset)))
        .when(left, |d| d.child(edge("resize-l", ResizeEdge::Left, CursorStyle::ResizeLeftRight).top_0().bottom_0().left_0().w(inset)))
        .when(right, |d| d.child(edge("resize-r", ResizeEdge::Right, CursorStyle::ResizeLeftRight).top_0().bottom_0().right_0().w(inset)))
        .when(top && left, |d| d.child(edge("resize-tl", ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight).top_0().left_0().size(inset * 2.)))
        .when(top && right, |d| d.child(edge("resize-tr", ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft).top_0().right_0().size(inset * 2.)))
        .when(bottom && left, |d| d.child(edge("resize-bl", ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().size(inset * 2.)))
        .when(bottom && right, |d| d.child(edge("resize-br", ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().size(inset * 2.)))
        .into_any_element()
}

fn control(id: &'static str, close: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded_full()
        .text_color(c(FG_DARK))
        .cursor_pointer()
        .map(|d| if close { d.hover(|s| s.bg(alpha(RED, 0.85)).text_color(c(BG_DARKER))) } else { d.hover(|s| s.bg(c(BG_HIGHLIGHT))) })
}

/// Minimize, maximize and close buttons, for the right end of the top bar.
pub fn window_controls(window: &Window) -> Option<AnyElement> {
    if !client_side(window) {
        return None;
    }
    let supported = window.window_controls();
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .ml(px(10.))
            .when(supported.minimize, |d| {
                d.child(
                    control("win-min", false)
                        .child(div().w(px(10.)).h(px(1.5)).bg(c(FG_DARK)))
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(supported.maximize, |d| {
                d.child(
                    control("win-max", false)
                        .child(div().size(px(10.)).rounded(px(1.5)).border_1().border_color(c(FG_DARK)))
                        .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(control("win-close", true).text_size(px(12.)).child("✕").on_click(|_, window, _| window.remove_window()))
            .into_any_element(),
    )
}
