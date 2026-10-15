//! Critters as avatars: `ainet-critter`'s 16x16 sprites painted as crisp
//! quads at any size, in the agent's colour from the theme (in Deepharness
//! and Omarchy, exactly the TUI's colours). They move with the fleet's tick:
//! a view that draws one keeps an [`Animation`], and redraws on the next
//! tick only if a critter was painted since, so hidden ones cost nothing.
//! Used in Fleet rows, the Agent panel and tabs; messages and toasts next.

use std::cell::Cell;
use std::rc::Rc;

use ainet_core::{Agent, AgentState};
use ainet_critter::{Colors, Grid, N, Pose, Rgb};
use gpui::{Bounds, Hsla, Pixels, Point, Rgba, canvas, fill, point, size};
use theme::Appearance;
use ui::prelude::*;

use crate::themes;

/// A run of same-coloured pixels on one row of a sprite: one quad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub x: usize,
    pub y: usize,
    pub len: usize,
    pub color: Rgb,
}

/// A sprite as runs, row by row, left to right: far fewer quads than
/// pixels, and no seams between neighbours of one colour.
pub fn runs(grid: &Grid) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        for (x, px) in row.iter().enumerate() {
            let Some(color) = *px else { continue };
            match out.last_mut() {
                Some(run) if run.y == y && run.x + run.len == x && run.color == color => run.len += 1,
                _ => out.push(Run { x, y, len: 1, color }),
            }
        }
    }
    out
}

/// Where a sprite is painted in `bounds`: its top left corner and the size
/// of one sprite pixel, both whole device pixels, so every pixel is the
/// same size and edges stay sharp. Centred in the bounds.
pub fn layout(bounds: Bounds<Pixels>, scale_factor: f32) -> (Point<Pixels>, Pixels) {
    let side = bounds.size.width.min(bounds.size.height).as_f32() * scale_factor;
    let device = (side / N as f32).floor().max(1.0);
    let cell = device / scale_factor;
    let sprite = cell * N as f32;
    let snap = |at: Pixels, room: Pixels| {
        let offset = ((room.as_f32() - sprite) / 2.0 * scale_factor).round() / scale_factor;
        px(((at.as_f32() * scale_factor).round() / scale_factor) + offset.max(0.0))
    };
    (
        point(
            snap(bounds.origin.x, bounds.size.width),
            snap(bounds.origin.y, bounds.size.height),
        ),
        px(cell),
    )
}

/// The quads of a sprite drawn at `origin` with pixels `cell` wide.
pub fn quads(grid: &Grid, origin: Point<Pixels>, cell: Pixels) -> Vec<(Bounds<Pixels>, Rgb)> {
    runs(grid)
        .into_iter()
        .map(|r| {
            let at = point(origin.x + cell * r.x as f32, origin.y + cell * r.y as f32);
            (Bounds::new(at, size(cell * r.len as f32, cell)), r.color)
        })
        .collect()
}

/// The colours `id`'s critter wears in the active theme: its palette's
/// when the theme was made from one (Deepharness, Omarchy), so it looks as
/// in the TUI; otherwise its player colour, with eyes and outline leaning
/// to the background (to the text on a light theme, so they still show).
pub fn colors(id: &str, cx: &App) -> Colors {
    if let Some(palette) = themes::palette(cx) {
        return palette.critter_colors(id);
    }
    let theme = cx.theme();
    let dark = match theme.appearance {
        Appearance::Dark => theme.colors().editor_background,
        Appearance::Light => theme.colors().text,
    };
    Colors {
        bodies: vec![to_rgb(themes::agent_color(id, cx).cursor)],
        dark: to_rgb(dark),
        ..Colors::default()
    }
}

pub fn to_rgb(color: Hsla) -> Rgb {
    let c = Rgba::from(color);
    [c.r, c.g, c.b].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn to_hsla(c: Rgb) -> Hsla {
    Rgba {
        r: f32::from(c[0]) / 255.0,
        g: f32::from(c[1]) / 255.0,
        b: f32::from(c[2]) / 255.0,
        a: 1.0,
    }
    .into()
}

/// Whether a view painted a critter since the last tick. Cheap to clone
/// into elements; the view keeps one and asks it on each tick.
#[derive(Clone, Default)]
pub struct Animation(Rc<Cell<bool>>);

impl Animation {
    fn painted(&self) {
        self.0.set(true);
    }

    /// True once after a critter was painted: the view should redraw.
    pub fn take(&self) -> bool {
        self.0.replace(false)
    }
}

/// A critter, `size` square (16 px by default).
#[derive(IntoElement)]
pub struct Critter {
    seed: u32,
    colors: Colors,
    pose: Pose,
    /// Greyed out in this colour: the agent is gone.
    ghost: Option<Hsla>,
    size: Pixels,
    animation: Option<Animation>,
}

impl Critter {
    /// `id`'s critter at `tick`, hopping about while `working`.
    pub fn new(id: &str, tick: u64, working: bool, cx: &App) -> Self {
        let seed = ainet_critter::hash(id);
        Critter {
            seed,
            colors: colors(id, cx),
            pose: Pose::at(seed, tick, working),
            ghost: None,
            size: px(N as f32),
            animation: None,
        }
    }

    /// An agent's critter: busy while it works, a still grey ghost once
    /// it is gone, as in the TUI.
    pub fn agent(agent: &Agent, tick: u64, cx: &App) -> Self {
        if agent.state == AgentState::Gone {
            let mut critter = Critter::new(&agent.id, 0, false, cx);
            critter.ghost = Some(cx.theme().colors().text_disabled);
            critter
        } else {
            Critter::new(&agent.id, tick, agent.state == AgentState::Working, cx)
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// Tells `animation` when painted, so the view redraws on the next
    /// tick. Ghosts stay still and do not.
    pub fn animate(mut self, animation: &Animation) -> Self {
        if self.ghost.is_none() {
            self.animation = Some(animation.clone());
        }
        self
    }

    /// The sprite in its current pose.
    pub fn grid(&self) -> Grid {
        ainet_critter::pixels(self.seed, &self.colors, self.pose)
    }
}

impl RenderOnce for Critter {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let grid = self.grid();
        let (ghost, animation) = (self.ghost, self.animation);
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if let Some(animation) = &animation {
                    animation.painted();
                }
                let (origin, cell) = layout(bounds, window.scale_factor());
                for (quad, color) in quads(&grid, origin, cell) {
                    let color = match ghost {
                        // Shades of one grey, so the shape still reads.
                        Some(grey) => grey.opacity(0.35 + 0.55 * luminance(color)),
                        None => to_hsla(color),
                    };
                    window.paint_quad(fill(quad, color));
                }
            },
        )
        .flex_none()
        .size(self.size)
    }
}

fn luminance(c: Rgb) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

#[cfg(test)]
mod tests {
    use ainet_critter::Pose;

    use super::*;

    fn sprite(id: &str, tick: u64) -> Grid {
        let seed = ainet_critter::hash(id);
        let colors = Colors::with_body([0x66, 0xcb, 0x8e]);
        ainet_critter::pixels(seed, &colors, Pose::at(seed, tick, true))
    }

    #[test]
    fn runs_cover_exactly_the_painted_pixels() {
        for id in ["a", "orch-lead", "empty-den"] {
            let grid = sprite(id, 3);
            let mut seen = [[None; N]; N];
            for r in runs(&grid) {
                assert!(r.len > 0 && r.x + r.len <= N);
                for px in &mut seen[r.y][r.x..r.x + r.len] {
                    assert!(px.is_none(), "pixels are painted once");
                    *px = Some(r.color);
                }
            }
            assert_eq!(seen, grid, "{id}");
            // Neighbours of one colour are one quad.
            let runs = runs(&grid);
            assert!(
                runs.windows(2)
                    .all(|w| !(w[0].y == w[1].y && w[0].x + w[0].len == w[1].x && w[0].color == w[1].color))
            );
        }
    }

    #[test]
    fn quads_are_deterministic_and_on_the_pixel_grid() {
        let grid = sprite("orch-lead", 7);
        let at = point(px(10.), px(20.));
        let first = quads(&grid, at, px(2.));
        assert_eq!(first, quads(&sprite("orch-lead", 7), at, px(2.)));
        assert_ne!(first, quads(&sprite("dev-ui", 7), at, px(2.)));
        let r = runs(&grid)[0];
        let (bounds, color) = first[0];
        assert_eq!(color, r.color);
        assert_eq!(
            bounds.origin,
            point(px(10. + 2. * r.x as f32), px(20. + 2. * r.y as f32))
        );
        assert_eq!(bounds.size, size(px(2. * r.len as f32), px(2.)));
        // Everything inside the sprite's 32 px square.
        assert!(
            first
                .iter()
                .all(|(b, _)| b.right() <= px(42.) && b.bottom() <= px(52.))
        );
    }

    #[test]
    fn sprites_snap_to_whole_device_pixels() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(40.), px(40.)));
        // 40 px holds 2 px a pixel (32 px), centred.
        assert_eq!(layout(bounds, 1.0), (point(px(4.), px(4.)), px(2.)));
        // At 1.5x, 60 device pixels hold 3 a pixel: 2 logical px.
        let (origin, cell) = layout(bounds, 1.5);
        assert_eq!(cell, px(2.));
        assert_eq!(origin.x.as_f32() * 1.5, (origin.x.as_f32() * 1.5).round());
        // Too small still draws, one device pixel a pixel.
        let tiny = Bounds::new(point(px(0.5), px(0.)), size(px(8.), px(8.)));
        let (origin, cell) = layout(tiny, 1.0);
        assert_eq!(cell, px(1.));
        assert_eq!(origin.x, px(1.), "snapped to a device pixel");
    }

    #[gpui::test]
    fn deepharness_critters_wear_the_tuis_colours(cx: &mut gpui::TestAppContext) {
        crate::tests::init_test(cx);
        cx.update(|cx| {
            let tui = ainet_model::Palette::default();
            for id in ["a1", "orch-lead", "3f2a9c"] {
                let ours = colors(id, cx);
                let theirs = tui.critter_colors(id);
                assert_eq!(ours.bodies, theirs.bodies, "{id}");
                assert_eq!((ours.dark, ours.blush), (theirs.dark, theirs.blush));
                // The theme's player colour is the same hue.
                assert_eq!(to_rgb(themes::agent_color(id, cx).cursor), tui.hue(id));
            }
            // Another theme: the player colour, eyes towards its text.
            crate::settings::reload(r#"{ "theme": "One Light" }"#, cx);
            let light = colors("a1", cx);
            assert_eq!(light.bodies, [to_rgb(themes::agent_color("a1", cx).cursor)]);
            assert_eq!(light.dark, to_rgb(cx.theme().colors().text));
        });
    }

    #[test]
    fn colours_round_trip() {
        for c in [[0, 0, 0], [0xea, 0x9e, 0x9e], [0x66, 0xcb, 0x8e], [255, 255, 255]] {
            assert_eq!(to_rgb(to_hsla(c)), c);
        }
    }

    #[test]
    fn the_animation_flag_is_taken_once() {
        let a = Animation::default();
        assert!(!a.take());
        a.painted();
        assert!(a.clone().take());
        assert!(!a.take());
    }
}
