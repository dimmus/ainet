//! Client-side window decorations on Linux, as Zed draws them
//! (`workspace::client_side_decorations` at the pinned tag): a transparent
//! margin that casts the window's shadow and catches resize drags, rounded
//! corners where the window is not tiled, and a one-pixel border.

use gpui::{
    Bounds, CursorStyle, Decorations, Global, HitboxBehavior, MouseButton, Pixels, Point, ResizeEdge, Size,
    Stateful, Tiling, black, canvas, point, px, size, transparent_black,
};
use theme::{CLIENT_SIDE_DECORATION_BORDER, CLIENT_SIDE_DECORATION_SHADOW, ClientDecorationsExt};
use ui::prelude::*;

/// Wraps the window's content in its decorations when the window manager
/// left them to us; with server-side decorations it is a plain container.
pub fn client_side_decorations(
    element: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let decorations = window.window_decorations();
    let is_resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    window.set_client_inset(match decorations {
        Decorations::Client { .. } => CLIENT_SIDE_DECORATION_SHADOW,
        Decorations::Server => px(0.0),
    });

    struct GlobalResizeEdge(ResizeEdge);
    impl Global for GlobalResizeEdge {}

    let shadow = CLIENT_SIDE_DECORATION_SHADOW;
    div()
        .id("window-backdrop")
        .bg(transparent_black())
        .map(|div| match decorations {
            Decorations::Server => div,
            Decorations::Client { .. } => div
                .rounded_client_corners(tiling)
                .when(!tiling.top, |div| div.pt(shadow))
                .when(!tiling.bottom, |div| div.pb(shadow))
                .when(!tiling.left, |div| div.pl(shadow))
                .when(!tiling.right, |div| div.pr(shadow))
                .when(is_resizable, |div| {
                    div.on_mouse_move(move |e, window, cx| {
                        let size = window.window_bounds().get_bounds().size;
                        let edge = resize_edge(e.position, shadow, size, tiling);
                        if edge != cx.try_global::<GlobalResizeEdge>().map(|g| g.0) {
                            // Re-render so the cursor follows the edge.
                            window.refresh();
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |e, window, _| {
                        let size = window.window_bounds().get_bounds().size;
                        if let Some(edge) = resize_edge(e.position, shadow, size, tiling) {
                            window.start_window_resize(edge);
                        }
                    })
                }),
        })
        .size_full()
        .child(
            div()
                .cursor(CursorStyle::Arrow)
                .map(|div| match decorations {
                    Decorations::Server => div,
                    Decorations::Client { .. } => div
                        .border_color(cx.theme().colors().border)
                        .rounded_client_corners(tiling)
                        .when(!tiling.top, |div| div.border_t(CLIENT_SIDE_DECORATION_BORDER))
                        .when(!tiling.bottom, |div| div.border_b(CLIENT_SIDE_DECORATION_BORDER))
                        .when(!tiling.left, |div| div.border_l(CLIENT_SIDE_DECORATION_BORDER))
                        .when(!tiling.right, |div| div.border_r(CLIENT_SIDE_DECORATION_BORDER))
                        .when(!tiling.is_tiled(), |div| {
                            // Zed's window shadow; themes do not define one.
                            div.shadow(vec![
                                gpui::BoxShadow::new(px(0.), px(0.), black().opacity(0.4))
                                    .blur_radius(shadow / 2.),
                            ])
                        }),
                })
                .on_mouse_move(|_e, _, cx| cx.stop_propagation())
                .size_full()
                .child(element),
        )
        .map(|div| match decorations {
            Decorations::Client { tiling } if is_resizable => div.child(
                canvas(
                    |_bounds, window, _| {
                        window.insert_hitbox(
                            Bounds::new(point(px(0.0), px(0.0)), window.window_bounds().get_bounds().size),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_bounds, hitbox, window, cx| {
                        let mouse = window.mouse_position();
                        let size = window.window_bounds().get_bounds().size;
                        let Some(edge) = resize_edge(mouse, shadow, size, tiling) else {
                            return;
                        };
                        cx.set_global(GlobalResizeEdge(edge));
                        window.set_cursor_style(
                            match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    CursorStyle::ResizeUpLeftDownRight
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    CursorStyle::ResizeUpRightDownLeft
                                }
                            },
                            &hitbox,
                        );
                    },
                )
                .size_full()
                .absolute(),
            ),
            _ => div,
        })
}

/// Which edge or corner a point in the shadow margin resizes, if any.
fn resize_edge(
    pos: Point<Pixels>,
    shadow: Pixels,
    window: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let inner = Bounds::new(Point::default(), window).inset(shadow * 1.5);
    if inner.contains(&pos) {
        return None;
    }
    let corner = size(shadow * 1.5, shadow * 1.5);
    let at = |x, y| Bounds::new(point(x, y), corner).contains(&pos);
    let (right, bottom) = (window.width - corner.width, window.height - corner.height);
    if !tiling.top && at(px(0.), px(0.)) {
        Some(ResizeEdge::TopLeft)
    } else if !tiling.top && at(right, px(0.)) {
        Some(ResizeEdge::TopRight)
    } else if !tiling.bottom && at(px(0.), bottom) {
        Some(ResizeEdge::BottomLeft)
    } else if !tiling.bottom && at(right, bottom) {
        Some(ResizeEdge::BottomRight)
    } else if !tiling.top && pos.y < shadow {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > window.height - shadow {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < shadow {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > window.width - shadow {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners() {
        let window = size(px(800.), px(600.));
        let shadow = px(10.);
        let free = Tiling::default();
        let edge = |x: f32, y: f32| resize_edge(point(px(x), px(y)), shadow, window, free);
        assert_eq!(edge(400., 300.), None);
        assert_eq!(edge(2., 2.), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(798., 2.), Some(ResizeEdge::TopRight));
        assert_eq!(edge(2., 598.), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(798., 598.), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(400., 3.), Some(ResizeEdge::Top));
        assert_eq!(edge(400., 597.), Some(ResizeEdge::Bottom));
        assert_eq!(edge(3., 300.), Some(ResizeEdge::Left));
        assert_eq!(edge(797., 300.), Some(ResizeEdge::Right));
        let tiled = Tiling {
            top: true,
            left: true,
            right: true,
            bottom: true,
        };
        assert_eq!(resize_edge(point(px(3.), px(300.)), shadow, window, tiled), None);
    }
}
