//! Draggable inner edges of the left sidebar and the right panel. A drag
//! resizes the pane (the graph or file view in between takes the rest), a
//! double-click puts it back to its default width, and the width is kept in
//! the projects file.

use gpui::{
    ClickEvent, Context, CursorStyle, DragMoveEvent, Empty, MouseButton, MouseDownEvent, Stateful, Window, div,
    prelude::*, px,
};

use super::app::GitPanda;
use super::theme::*;

/// Narrowest a side pane can be dragged.
pub const MIN_W: f32 = 180.;
/// Room always left for the graph or file view between the panes.
pub const CENTER_MIN_W: f32 = 320.;
/// Width of the grabbable strip along a pane's edge.
const HANDLE_W: f32 = 6.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// What is dragged: a pane's inner edge.
#[derive(Clone, Copy, Debug)]
pub struct PaneEdge(pub Side);

/// Where an edge was pressed: the cursor's x and the pane's width then, so
/// the edge moves with the cursor instead of jumping to it.
#[derive(Clone, Copy, Debug)]
pub struct PaneGrab {
    x: f32,
    width: f32,
}

impl Render for PaneEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// A width for one pane that fits beside `other` in `total`.
pub fn clamp_width(w: f32, other: f32, total: f32) -> f32 {
    w.min(total - other - CENTER_MIN_W).max(MIN_W)
}

impl GitPanda {
    pub fn pane_width(&self, side: Side) -> f32 {
        match side {
            Side::Left => self.workspace.sidebar.unwrap_or(super::sidebar::WIDTH),
            Side::Right => self.workspace.panel.unwrap_or(super::panel::WIDTH),
        }
    }

    /// The strip along `side`'s inner edge; place it in the pane, which must
    /// be `relative`.
    pub fn pane_handle(&self, side: Side, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let (id, name) = match side {
            Side::Left => ("sidebar-edge", "sidebar-edge"),
            Side::Right => ("panel-edge", "panel-edge"),
        };
        div()
            .id(id)
            .debug_selector(move || name.into())
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(HANDLE_W))
            .map(|d| match side {
                Side::Left => d.right_0(),
                Side::Right => d.left_0(),
            })
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|s| s.bg(alpha(BLUE, 0.35)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, _| {
                    this.pane_grab = Some(PaneGrab { x: f32::from(ev.position.x), width: this.pane_width(side) });
                }),
            )
            .on_drag(PaneEdge(side), |edge, _, _, cx| cx.new(|_| *edge))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() == 2 {
                    match side {
                        Side::Left => this.workspace.sidebar = None,
                        Side::Right => this.workspace.panel = None,
                    }
                    this.save_workspace(cx);
                    cx.notify();
                }
            }))
    }

    /// Follows a dragged edge; `ev.bounds` is the row holding both panes.
    pub fn on_pane_drag(&mut self, ev: &DragMoveEvent<PaneEdge>, _: &mut Window, cx: &mut Context<Self>) {
        let side = ev.drag(cx).0;
        let Some(grab) = self.pane_grab else { return };
        let dx = f32::from(ev.event.position.x) - grab.x;
        let total = f32::from(ev.bounds.size.width);
        match side {
            Side::Left => {
                let w = clamp_width(grab.width + dx, self.pane_width(Side::Right), total);
                self.workspace.sidebar = Some(w.round());
            }
            Side::Right => {
                let w = clamp_width(grab.width - dx, self.pane_width(Side::Left), total);
                self.workspace.panel = Some(w.round());
            }
        }
        cx.notify();
    }

    /// Ends a drag: keeps the new width.
    pub fn on_pane_drop(&mut self, _: &PaneEdge, _: &mut Window, cx: &mut Context<Self>) {
        self.pane_grab = None;
        self.save_workspace(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_leave_room_for_the_center() {
        assert_eq!(clamp_width(300., 390., 1400.), 300.);
        assert_eq!(clamp_width(50., 390., 1400.), MIN_W);
        assert_eq!(clamp_width(2000., 390., 1400.), 1400. - 390. - CENTER_MIN_W);
        // A window too small for both wins over the center.
        assert_eq!(clamp_width(300., 390., 600.), MIN_W);
    }
}
