//! A vertical scrollbar for a `uniform_list`. gpui ships none, so this paints
//! a track and thumb on a canvas and drives the list's scroll handle directly.
//! It reads the handle at paint time, after the list has laid out, so the
//! thumb always matches the frame being drawn.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, CursorStyle, DispatchPhase, HitboxBehavior, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, UniformListScrollHandle, canvas, point, prelude::*, px,
    size,
};

use super::theme::*;

pub const SCROLLBAR_W: f32 = 10.;
const MIN_THUMB_H: f32 = 24.;
const THUMB_INSET: f32 = 2.;

/// Where the thumb was grabbed, as a distance from its top, while a drag is
/// in progress. Lives on the view so it survives between frames.
pub type ScrollbarDrag = Rc<Cell<Option<Pixels>>>;

struct Metrics {
    /// The list's visible bounds; the track runs its full height.
    viewport: Bounds<Pixels>,
    /// How far the list can scroll.
    max: Pixels,
    thumb_h: Pixels,
}

impl Metrics {
    fn of(handle: &UniformListScrollHandle) -> Option<Self> {
        let base = handle.0.borrow().base_handle.clone();
        let viewport = base.bounds();
        let max = base.max_offset().height;
        if max <= px(0.) || viewport.size.height <= px(0.) {
            return None;
        }
        let view_h = viewport.size.height;
        let thumb_h = (view_h * (view_h / (view_h + max))).max(px(MIN_THUMB_H)).min(view_h);
        Some(Self { viewport, max, thumb_h })
    }

    /// Thumb top relative to the track, for the list's current offset.
    fn thumb_top(&self, handle: &UniformListScrollHandle) -> Pixels {
        let scrolled = -handle.0.borrow().base_handle.offset().y;
        let travel = self.viewport.size.height - self.thumb_h;
        travel * (scrolled / self.max).clamp(0., 1.)
    }

    /// Scrolls so the thumb's top lands at `top` (relative to the track).
    fn scroll_to_thumb_top(&self, handle: &UniformListScrollHandle, top: Pixels) {
        let travel = self.viewport.size.height - self.thumb_h;
        let frac = if travel > px(0.) { (top / travel).clamp(0., 1.) } else { 0. };
        let base = handle.0.borrow().base_handle.clone();
        let x = base.offset().x;
        base.set_offset(point(x, -(self.max * frac)));
    }
}

/// A full-height scrollbar meant to sit beside the list it controls. Clicking
/// the track jumps the thumb there; dragging the thumb scrolls the list.
pub fn scrollbar(handle: UniformListScrollHandle, drag: ScrollbarDrag) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::BlockMouse),
        move |bounds, hitbox, window, _| {
            let Some(m) = Metrics::of(&handle) else {
                drag.set(None);
                return;
            };
            let track = Bounds::new(bounds.origin, size(bounds.size.width, m.viewport.size.height));
            let thumb_top = m.thumb_top(&handle);
            let thumb = Bounds::new(
                point(track.left() + px(THUMB_INSET), track.top() + thumb_top),
                size(track.size.width - px(THUMB_INSET * 2.), m.thumb_h),
            );

            let hovered = hitbox.is_hovered(window);
            let dragging = drag.get().is_some();
            let color = if dragging {
                alpha(DARK5, 0.9)
            } else if hovered {
                alpha(DARK5, 0.7)
            } else {
                alpha(TERMINAL_BLACK, 0.8)
            };
            window.paint_quad(gpui::fill(track, alpha(BG_DARK, 0.6)));
            window.paint_quad(gpui::fill(thumb, color).corner_radii(px(3.)));
            window.set_cursor_style(CursorStyle::Arrow, &hitbox);

            window.on_mouse_event({
                let (handle, drag, hitbox) = (handle.clone(), drag.clone(), hitbox.clone());
                move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || ev.button != MouseButton::Left
                        || !hitbox.is_hovered(window)
                    {
                        return;
                    }
                    let Some(m) = Metrics::of(&handle) else { return };
                    let y = ev.position.y - track.top();
                    let top = m.thumb_top(&handle);
                    let grab = if y >= top && y <= top + m.thumb_h {
                        y - top
                    } else {
                        // Track click: center the thumb on the pointer.
                        let grab = m.thumb_h / 2.;
                        m.scroll_to_thumb_top(&handle, y - grab);
                        grab
                    };
                    drag.set(Some(grab));
                    cx.stop_propagation();
                    window.refresh();
                }
            });
            window.on_mouse_event({
                let (handle, drag) = (handle.clone(), drag.clone());
                move |ev: &MouseMoveEvent, phase, window, cx| {
                    let Some(grab) = drag.get() else { return };
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if !ev.dragging() {
                        drag.set(None);
                        window.refresh();
                        return;
                    }
                    if let Some(m) = Metrics::of(&handle) {
                        m.scroll_to_thumb_top(&handle, ev.position.y - track.top() - grab);
                    }
                    cx.stop_propagation();
                    window.refresh();
                }
            });
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left && drag.take().is_some() {
                    window.refresh();
                }
            });
        },
    )
    .w(px(SCROLLBAR_W))
    .h_full()
    .flex_none()
}
