//! The Map: the fleet as a picture, the GUI's version of the TUI's Map
//! (`draw_map` in the TUI's `view.rs`). Each rig is a labelled cluster of
//! its live agents, the loose agents a cluster of their own, and who
//! directs whom runs as curved edges with arrowheads. The layout is
//! `ainet_model::map::layout`, the TUI's layers by depth per cluster, fitted
//! to the tab.
//!
//! What moves: the critters; the TUI's work in flight along each edge
//! (down to an agent at work, a question up from one that needs you,
//! `map::pulses`, on the fleet's tick); and every new message between two
//! agents on the map, a dot flying from sender to recipient along their
//! edge (or straight across) over [`FLIGHT`], drawn frame by frame while
//! any is in the air ([`Flights`]).
//!
//! Hovering a node lights it and its edges; a click selects the agent (the
//! Fleet and Agent panels follow) and a double click opens its terminal.
//! The wheel pans, as does dragging the background; `ctrl`-wheel (`cmd` on
//! macOS) and `map::ZoomIn`/`ZoomOut` zoom; `map::FitView` fits the whole
//! fleet again, as it is at first.

use std::time::{Duration, Instant};

use ainet_core::{Agent, AgentState, EdgeKind, Message};
use ainet_model::map::{self as layout, Layout};
use gpui::{
    Action, AnyElement, AvailableSpace, BorderStyle, Bounds, ClickEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder, Point,
    ScrollWheelEvent, Subscription, canvas, point, quad, size,
};
use ui::{Indicator, Tooltip, prelude::*};

use crate::actions::{FitView, OpenTerminal, ZoomIn, ZoomOut};
use crate::comms::hue;
use crate::critter::{self, Critter};
use crate::fleet::{FleetModel, Tick};
use crate::item::{Item, ItemEvent};
use crate::panels::state_color;
use crate::persistence::SavedItem;

/// How long a message takes to fly from sender to recipient.
pub const FLIGHT: Duration = Duration::from_millis(1000);
/// At most this many messages in the air; older ones land at once.
const MAX_FLIGHTS: usize = 64;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
/// What one `ZoomIn` multiplies the zoom by.
const ZOOM_STEP: f32 = 1.25;
/// Room between the tab's edge and the clusters.
const MARGIN: Pixels = px(8.);

/// A message in the air: from one agent to another, since `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flight {
    pub message: u64,
    pub from: String,
    pub to: String,
    pub start: Instant,
}

/// The messages in the air. It notes the newest message it has seen, so
/// each new one between two agents on the map flies once; messages from
/// before the map was opened, and edits of old ones (delivered, read), do
/// not fly. Neither do those a snapshot brings (ainetd's history, which
/// arrives after the map may have opened, or what came while
/// reconnecting): each snapshot starts the watch anew.
#[derive(Clone, Debug, Default)]
pub struct Flights {
    seen: Option<u64>,
    /// The snapshot `seen` was noted in (`FleetModel::snapshots`).
    snapshot: u64,
    flights: Vec<Flight>,
}

impl Flights {
    /// Takes off every message newer than any seen whose sender and
    /// recipient are on the map (`on_map`), at `now`; the first call, and
    /// the first after another snapshot (`snapshot` counts them), only
    /// notes where the stream is. Returns how many took off.
    pub fn watch(
        &mut self,
        messages: &[Message],
        snapshot: u64,
        on_map: impl Fn(&str) -> bool,
        now: Instant,
    ) -> usize {
        let newest = messages.iter().map(|m| m.id).max();
        let seen = match self.seen {
            Some(seen) if snapshot == self.snapshot => seen,
            _ => {
                self.seen = Some(newest.unwrap_or(0));
                self.snapshot = snapshot;
                return 0;
            }
        };
        let before = self.flights.len();
        for m in messages.iter().filter(|m| m.id > seen) {
            if m.from != m.to && on_map(&m.from) && on_map(&m.to) {
                self.flights.push(Flight {
                    message: m.id,
                    from: m.from.clone(),
                    to: m.to.clone(),
                    start: now,
                });
            }
        }
        self.seen = Some(newest.unwrap_or(seen).max(seen));
        let excess = self.flights.len().saturating_sub(MAX_FLIGHTS);
        self.flights.drain(..excess);
        self.flights.len() - before.min(self.flights.len())
    }

    /// How far along `flight` is at `now`, 0 to 1, easing in and out.
    pub fn progress(flight: &Flight, now: Instant) -> f32 {
        let t = (now.saturating_duration_since(flight.start).as_secs_f32() / FLIGHT.as_secs_f32()).min(1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Drops the flights that have arrived by `now`.
    pub fn land(&mut self, now: Instant) {
        self.flights
            .retain(|f| now.saturating_duration_since(f.start) < FLIGHT);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Flight> {
        self.flights.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.flights.is_empty()
    }
}

/// How the map is looked at: zoomed about the middle of its area, then
/// moved by `pan`. The default fits the whole fleet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub zoom: f32,
    pub pan: Point<Pixels>,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            zoom: 1.0,
            pan: Point::default(),
        }
    }
}

impl Camera {
    /// Where layout point `p` of a layout made for `area` shows.
    pub fn to_screen(self, area: Bounds<Pixels>, p: layout::Point) -> Point<Pixels> {
        let c = area.center();
        point(
            c.x + (area.origin.x + px(p.x) - c.x) * self.zoom + self.pan.x,
            c.y + (area.origin.y + px(p.y) - c.y) * self.zoom + self.pan.y,
        )
    }

    /// The layout point that shows at `at`.
    pub fn to_layout(self, area: Bounds<Pixels>, at: Point<Pixels>) -> layout::Point {
        let c = area.center();
        layout::Point::new(
            ((at.x - c.x - self.pan.x) / self.zoom + c.x - area.origin.x).as_f32(),
            ((at.y - c.y - self.pan.y) / self.zoom + c.y - area.origin.y).as_f32(),
        )
    }

    /// Where layout rectangle `r` shows.
    pub fn rect(self, area: Bounds<Pixels>, r: &layout::Rect) -> Bounds<Pixels> {
        Bounds::new(
            self.to_screen(area, layout::Point::new(r.x, r.y)),
            size(px(r.w * self.zoom), px(r.h * self.zoom)),
        )
    }

    /// Zooms by `factor`, keeping what shows at `at` where it is.
    pub fn zoom_at(&mut self, area: Bounds<Pixels>, at: Point<Pixels>, factor: f32) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let f = zoom / self.zoom;
        let c = area.center();
        self.pan = point(
            (at.x - c.x) - (at.x - c.x - self.pan.x) * f,
            (at.y - c.y) - (at.y - c.y - self.pan.y) * f,
        );
        self.zoom = zoom;
    }
}

/// An edge as drawn: its curve on screen, colour, width, dashed for
/// `spawned_by`.
struct EdgeShape {
    curve: [Point<Pixels>; 4],
    color: Hsla,
    width: Pixels,
    dashed: bool,
}

/// Everything the map paints, worked out before painting.
struct Scene {
    groups: Vec<Bounds<Pixels>>,
    edges: Vec<EdgeShape>,
    /// Dots: where, how big across, and their colour.
    dots: Vec<(Point<Pixels>, Pixels, Hsla)>,
    /// Cluster titles and nodes, laid out and placed.
    elements: Vec<AnyElement>,
}

pub struct MapItem {
    fleet: Entity<FleetModel>,
    focus_handle: FocusHandle,
    camera: Camera,
    hovered: Option<String>,
    flights: Flights,
    animation: critter::Animation,
    /// What was drawn last: the area laid out and its layout, for the
    /// mouse and for tests.
    drawn: Option<(Bounds<Pixels>, Layout)>,
    /// A drag on the background panning the map: where it started, and
    /// the pan then.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    _subscriptions: Vec<Subscription>,
}

impl MapItem {
    pub fn new(fleet: Entity<FleetModel>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&fleet, |this, _, cx| {
                this.watch(cx);
                cx.notify();
            }),
            // The critters and the TUI's work in flight move on the tick.
            cx.subscribe(&fleet, |this: &mut Self, _, _: &Tick, cx| {
                if this.animation.take() {
                    cx.notify();
                }
            }),
        ];
        let mut map = MapItem {
            fleet,
            focus_handle: cx.focus_handle(),
            camera: Camera::default(),
            hovered: None,
            flights: Flights::default(),
            animation: critter::Animation::default(),
            drawn: None,
            drag: None,
            _subscriptions: subscriptions,
        };
        map.watch(cx);
        map
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn flights(&self) -> &Flights {
        &self.flights
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Looks at the map as `camera` does (a saved view), within the zoom's
    /// limits.
    pub fn set_camera(&mut self, camera: Camera, cx: &mut Context<Self>) {
        let finite = [camera.zoom, camera.pan.x.as_f32(), camera.pan.y.as_f32()]
            .iter()
            .all(|v| v.is_finite());
        if finite {
            self.camera = Camera {
                zoom: camera.zoom.clamp(MIN_ZOOM, MAX_ZOOM),
                pan: camera.pan,
            };
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }

    /// The area last laid out and its layout.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn drawn(&self) -> Option<&(Bounds<Pixels>, Layout)> {
        self.drawn.as_ref()
    }

    /// New messages between agents on the map take off.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let fleet = self.fleet.read(cx);
        let store = &fleet.store;
        let on_map = |id: &str| store.agents.get(id).is_some_and(|a| a.state.is_live());
        self.flights
            .watch(&store.messages, fleet.snapshots(), on_map, now);
    }

    fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.fleet.update(cx, |fleet, cx| fleet.select(id, cx));
    }

    fn zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some((area, _)) = &self.drawn {
            let area = *area;
            self.camera.zoom_at(area, area.center(), factor);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = e.delta.pixel_delta(px(20.));
        if e.modifiers.secondary() {
            if let Some((area, _)) = &self.drawn {
                let area = *area;
                let factor = (delta.y.as_f32() * 0.01).clamp(-1.0, 1.0).exp();
                self.camera.zoom_at(area, e.position, factor);
            }
        } else {
            self.camera.pan += delta;
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A press on the background starts panning; on a node, it is the
    /// node's click.
    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let on_node = self
            .drawn
            .as_ref()
            .is_some_and(|(area, layout)| layout.node_at(self.camera.to_layout(*area, e.position)).is_some());
        if !on_node {
            self.drag = Some((e.position, self.camera.pan));
        }
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((from, pan)) = self.drag else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        self.camera.pan = pan + (e.position - from);
        cx.notify();
    }

    /// Lays the fleet out in `bounds` and works out what to paint: the
    /// clusters, edges and dots, and the titles and nodes as elements.
    fn prepaint_map(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Scene {
        let area = Bounds::new(
            point(bounds.origin.x + MARGIN, bounds.origin.y + MARGIN),
            size(
                (bounds.size.width - MARGIN * 2.).max(px(1.)),
                (bounds.size.height - MARGIN * 2.).max(px(1.)),
            ),
        );
        let now = cx.background_executor().now();
        self.flights.land(now);

        let fleet = self.fleet.read(cx);
        let (store, tick) = (&fleet.store, fleet.tick());
        let map = layout::layout(store, area.size.width.as_f32(), area.size.height.as_f32());
        let agents: Vec<Agent> = map
            .nodes
            .iter()
            .filter_map(|n| store.agents.get(&n.id).cloned())
            .collect();
        let selected = store.selected.clone();
        let titles: Vec<(SharedString, SharedString, SharedString)> = map
            .groups
            .iter()
            .map(|g| match &g.rig {
                Some(rig) => {
                    let counts = store.rig_counts(rig);
                    let summary = store
                        .rigs
                        .get(rig)
                        .and_then(|r| r.summary.clone())
                        .unwrap_or_default();
                    (
                        rig.clone().into(),
                        format!("{}/{} running", counts.running, counts.seats).into(),
                        summary.into(),
                    )
                }
                None => (
                    "loose agents".into(),
                    format!("{} running", g.nodes.len()).into(),
                    SharedString::default(),
                ),
            })
            .collect();

        let camera = self.camera;
        let theme = cx.theme();
        let colors = theme.colors();
        let hovered = self.hovered.as_ref().and_then(|id| map.node(id));
        let shown = camera.zoom.clamp(0.75, 1.5);

        let groups = map.groups.iter().map(|g| camera.rect(area, &g.bounds)).collect();
        let curve = |c: [layout::Point; 4]| c.map(|p| camera.to_screen(area, p));
        let edges = map
            .edges
            .iter()
            .map(|e| {
                let lit = hovered.is_some_and(|h| map.touches(e, h));
                let color = match hovered {
                    Some(_) if lit => colors.text_accent,
                    Some(_) => colors.border_variant,
                    None => colors.border,
                };
                EdgeShape {
                    curve: curve(layout::route(&map.nodes[e.from].bounds, &map.nodes[e.to].bounds)),
                    color,
                    width: px(if lit { 2.5 } else { 1.5 }),
                    dashed: e.kind == Some(EdgeKind::SpawnedBy),
                }
            })
            .collect();

        let mut dots = Vec::new();
        // The TUI's work in flight: down to an agent at work, a question
        // up from one that needs you; a fading trail behind each.
        for e in &map.edges {
            let Some(child) = agents.get(e.to) else { continue };
            let (upward, color) = match child.state {
                AgentState::Working => (false, hue(&child.id, cx)),
                AgentState::NeedsYou => (true, theme.status().warning),
                _ => continue,
            };
            let c = layout::route(&map.nodes[e.from].bounds, &map.nodes[e.to].bounds);
            for t in layout::pulses(tick, &child.id, upward) {
                for (lag, opacity, r) in [(0.06, 0.25, 1.5), (0.03, 0.55, 2.0), (0.0, 1.0, 3.0)] {
                    let t = if upward {
                        (t + lag).min(1.0)
                    } else {
                        (t - lag).max(0.0)
                    };
                    let at = camera.to_screen(area, layout::bezier(&c, t as f32));
                    dots.push((at, px(r * shown), color.opacity(opacity)));
                }
            }
        }
        // Messages in flight, along their edge or straight across.
        for flight in self.flights.iter() {
            let (Some(a), Some(b)) = (map.node(&flight.from), map.node(&flight.to)) else {
                continue;
            };
            let p = Flights::progress(flight, now);
            // Along the edge (backwards for a message up it), or straight
            // across; `ahead` is which way along the curve it moves.
            let (c, t, ahead) = match map.edge_between(a, b) {
                Some(e) => {
                    let c = layout::route(&map.nodes[e.from].bounds, &map.nodes[e.to].bounds);
                    if e.from == a {
                        (c, p, 1.0)
                    } else {
                        (c, 1.0 - p, -1.0)
                    }
                }
                None => {
                    let (s, e) = (map.nodes[a].bounds.center(), map.nodes[b].bounds.center());
                    let at = |k: f32| layout::Point::new(s.x + (e.x - s.x) * k, s.y + (e.y - s.y) * k);
                    ([s, at(1.0 / 3.0), at(2.0 / 3.0), e], p, 1.0)
                }
            };
            let color = hue(&flight.from, cx);
            // A fading trail behind the message, then the message.
            for (k, (opacity, r)) in [(0.15, 2.0), (0.35, 2.5), (0.6, 3.0), (1.0, 4.5)]
                .into_iter()
                .enumerate()
            {
                let behind = (3 - k) as f32 * 0.035 * ahead;
                let at = camera.to_screen(area, layout::bezier(&c, (t - behind).clamp(0.0, 1.0)));
                dots.push((at, px(r * shown), color.opacity(opacity)));
            }
        }

        // The titles and nodes, as elements placed where the layout says.
        let mut elements = Vec::new();
        for (g, (name, counts, summary)) in map.groups.iter().zip(titles) {
            let r = camera.rect(area, &g.bounds);
            let height = px(layout::HEADER * map.scale * camera.zoom);
            if height < px(14.) {
                continue;
            }
            let icon = if g.rig.is_some() {
                IconName::Server
            } else {
                IconName::UserGroup
            };
            let mut title = self.render_title(icon, name, counts, summary, r.size.width, height);
            title.prepaint_as_root(
                r.origin,
                size(
                    AvailableSpace::Definite(r.size.width),
                    AvailableSpace::Definite(height),
                ),
                window,
                cx,
            );
            elements.push(title);
        }
        for (ix, (node, agent)) in map.nodes.iter().zip(&agents).enumerate() {
            let r = camera.rect(area, &node.bounds);
            let mut el = self.render_node(
                agent,
                r.size,
                map.scale * camera.zoom,
                selected.as_deref() == Some(agent.id.as_str()),
                hovered == Some(ix),
                tick,
                cx,
            );
            el.prepaint_as_root(
                r.origin,
                size(
                    AvailableSpace::Definite(r.size.width),
                    AvailableSpace::Definite(r.size.height),
                ),
                window,
                cx,
            );
            elements.push(el);
        }

        // Messages still in the air: another frame.
        if !self.flights.is_empty() {
            let id = cx.entity_id();
            window.on_next_frame(move |_, cx| cx.notify(id));
        }
        self.drawn = Some((area, map));
        Scene {
            groups,
            edges,
            dots,
            elements,
        }
    }

    fn render_title(
        &self,
        icon: IconName,
        name: SharedString,
        counts: SharedString,
        summary: SharedString,
        width: Pixels,
        height: Pixels,
    ) -> AnyElement {
        h_flex()
            .w(width)
            .h(height)
            .px_2()
            .gap_1p5()
            .overflow_hidden()
            .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
            .child(
                Label::new(name)
                    .size(LabelSize::Small)
                    .weight(FontWeight::SEMIBOLD)
                    .single_line(),
            )
            .child(
                Label::new(counts)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .single_line(),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(summary)
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder)
                        .truncate(),
                ),
            )
            .into_any_element()
    }

    /// An agent's node, `size` big at `scale` times its natural size: its
    /// critter, and with room its name, role (in its colour) and state.
    #[allow(clippy::too_many_arguments)]
    fn render_node(
        &self,
        agent: &Agent,
        size: gpui::Size<Pixels>,
        scale: f32,
        selected: bool,
        hovered: bool,
        tick: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let id = agent.id.clone();
        let (hover_id, click_id, selector) = (id.clone(), id.clone(), id.clone());
        let critter = px((32.0 * scale).clamp(10.0, 64.0));
        let line = |label: Label| {
            h_flex()
                .w_full()
                .min_w_0()
                .justify_center()
                .child(label.truncate())
        };
        let tooltip = {
            let mut text = agent.label();
            if let Some(role) = &agent.role {
                text = format!("{text} · {role}");
            }
            if let Some(task) = &agent.task {
                text = format!("{text}\n{task}");
            }
            text
        };
        v_flex()
            .id(SharedString::from(format!("map-node-{id}")))
            .debug_selector(move || format!("MAP-NODE-{selector}"))
            .w(size.width)
            .h(size.height)
            .p_1()
            .gap_0p5()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(if selected {
                colors.border_focused
            } else {
                gpui::transparent_black()
            })
            .when(hovered, |el| el.bg(colors.ghost_element_hover))
            .when(selected, |el| el.bg(colors.ghost_element_selected))
            .cursor_pointer()
            .child(
                Critter::agent(agent, tick, cx)
                    .size(critter)
                    .animate(&self.animation),
            )
            .when(scale >= 0.6, |el| {
                el.child(line(
                    Label::new(agent.name.clone())
                        .size(LabelSize::Small)
                        .when(selected, |l| l.weight(FontWeight::BOLD)),
                ))
            })
            .when(scale >= 0.8, |el| {
                el.when_some(agent.role.clone(), |el, role| {
                    el.child(line(
                        Label::new(role)
                            .size(LabelSize::XSmall)
                            .color(Color::Custom(hue(&agent.id, cx))),
                    ))
                })
                .child(
                    h_flex()
                        .gap_1()
                        .child(Indicator::dot().color(state_color(agent.state)))
                        .child(
                            Label::new(agent.state.label())
                                .size(LabelSize::XSmall)
                                .color(state_color(agent.state)),
                        ),
                )
            })
            .tooltip(Tooltip::text(tooltip))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(hover_id.clone());
                } else if this.hovered.as_deref() == Some(hover_id.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.select(&click_id, window, cx);
                if e.click_count() == 2 {
                    window.dispatch_action(OpenTerminal.boxed_clone(), cx);
                }
            }))
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let legend = |swatch: AnyElement, text: &'static str| {
            h_flex()
                .flex_none()
                .gap_1()
                .child(swatch)
                .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
        };
        let stroke = |w: Pixels| div().w(w).h(px(1.5)).bg(colors.border);
        let fitted = self.camera == Camera::default();
        h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                Icon::new(IconName::Blocks)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new("Map"))
            .child(
                Label::new("who directs whom")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .gap_3()
                    .overflow_hidden()
                    .child(legend(stroke(px(14.)).into_any_element(), "directs"))
                    .child(legend(
                        h_flex()
                            .gap(px(2.))
                            .child(stroke(px(4.)))
                            .child(stroke(px(4.)))
                            .child(stroke(px(4.)))
                            .into_any_element(),
                        "spawned",
                    ))
                    .child(legend(
                        Indicator::dot().color(Color::Success).into_any_element(),
                        "work flowing down",
                    ))
                    .child(legend(
                        Indicator::dot().color(Color::Warning).into_any_element(),
                        "a question flowing up",
                    ))
                    .child(legend(
                        Indicator::dot().color(Color::Accent).into_any_element(),
                        "a message",
                    )),
            )
            .child(
                Label::new(format!("{:.0}%", self.camera.zoom * 100.))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                IconButton::new("map-zoom-out", IconName::Dash)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::for_action_title("Zoom Out", &ZoomOut))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1. / ZOOM_STEP, cx))),
            )
            .child(
                IconButton::new("map-zoom-in", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::for_action_title("Zoom In", &ZoomIn))
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(ZOOM_STEP, cx))),
            )
            .child(
                IconButton::new("map-fit", IconName::Maximize)
                    .icon_size(IconSize::Small)
                    .disabled(fitted)
                    .tooltip(Tooltip::for_action_title("Fit to View", &FitView))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.camera = Camera::default();
                        cx.notify();
                    })),
            )
    }
}

/// Paints what [`MapItem::prepaint_map`] worked out: the clusters, the
/// edges with their arrowheads, the dots, then the titles and nodes.
fn paint_scene(scene: Scene, colors: (Hsla, Hsla), zoom: f32, window: &mut Window, cx: &mut App) {
    let (fill, border) = colors;
    for g in &scene.groups {
        window.paint_quad(quad(*g, px(8.), fill, px(1.), border, BorderStyle::Solid));
    }
    let head = zoom.clamp(0.75, 1.5);
    for e in &scene.edges {
        let [s, c1, c2, end] = e.curve;
        let mut builder = PathBuilder::stroke(e.width);
        if e.dashed {
            builder = builder.dash_array(&[px(5.), px(4.)]);
        }
        builder.move_to(s);
        builder.cubic_bezier_to(end, c1, c2);
        if let Ok(path) = builder.build() {
            window.paint_path(path, e.color);
        }
        // The arrowhead, along the curve's last tangent.
        let toward = if c2 == end { s } else { c2 };
        let (dx, dy) = ((end.x - toward.x).as_f32(), (end.y - toward.y).as_f32());
        let len = (dx * dx + dy * dy).sqrt();
        if len > f32::EPSILON {
            let (ux, uy) = (dx / len, dy / len);
            let (long, half) = (8.0 * head, 4.0 * head);
            let base = point(end.x - px(ux * long), end.y - px(uy * long));
            let mut arrow = PathBuilder::fill();
            arrow.add_polygon(
                &[
                    end,
                    point(base.x - px(uy * half), base.y + px(ux * half)),
                    point(base.x + px(uy * half), base.y - px(ux * half)),
                ],
                true,
            );
            if let Ok(path) = arrow.build() {
                window.paint_path(path, e.color);
            }
        }
    }
    for (at, r, color) in &scene.dots {
        let corner = point(at.x - *r, at.y - *r);
        window.paint_quad(quad(
            Bounds::new(corner, size(*r * 2., *r * 2.)),
            *r,
            *color,
            px(0.),
            gpui::transparent_black(),
            BorderStyle::Solid,
        ));
    }
    for mut el in scene.elements {
        el.paint(window, cx);
    }
}

impl EventEmitter<ItemEvent> for MapItem {}

impl Focusable for MapItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for MapItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        "Map".into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(IconName::Blocks)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("Who directs whom, and messages in flight".into())
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Map {
            zoom: self.camera.zoom,
            pan_x: self.camera.pan.x.as_f32(),
            pan_y: self.camera.pan.y.as_f32(),
        })
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Self>> {
        let fleet = self.fleet.clone();
        Some(cx.new(|cx| MapItem::new(fleet, cx)))
    }
}

impl Render for MapItem {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let (fill, border) = (colors.panel_background, colors.border_variant);
        let empty = !self
            .fleet
            .read(cx)
            .store
            .agents
            .values()
            .any(|a| a.state.is_live());
        let view = cx.entity();
        let zoom = self.camera.zoom;
        v_flex()
            .key_context("Map")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(colors.editor_background)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(ZOOM_STEP, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(1. / ZOOM_STEP, cx)))
            .on_action(cx.listener(|this, _: &FitView, _, cx| {
                this.camera = Camera::default();
                cx.notify();
            }))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .id("map-canvas")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                view.update(cx, |this, cx| this.prepaint_map(bounds, window, cx))
                            },
                            move |_, scene, window, cx| paint_scene(scene, (fill, border), zoom, window, cx),
                        )
                        .size_full(),
                    )
                    .when(empty, |el| {
                        el.child(
                            v_flex()
                                .absolute()
                                .inset_0()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .child(Label::new("Nobody is running yet").color(Color::Muted))
                                .child(
                                    Label::new("Rigs and agents show here once they start")
                                        .size(LabelSize::Small)
                                        .color(Color::Placeholder),
                                ),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use ainet_core::Event;
    use ainet_model::Msg;

    use super::*;

    fn message(id: u64, from: &str, to: &str) -> Message {
        Message {
            id,
            from: from.into(),
            from_label: from.into(),
            to: to.into(),
            to_label: to.into(),
            body: "hi".into(),
            reply_to: None,
            thread: id,
            created: 0,
            delivered: None,
            read: None,
            held: false,
        }
    }

    #[test]
    fn a_flight_takes_off_once_and_lands_after_a_second() {
        let t0 = Instant::now();
        let on_map = |id: &str| id != "operator";
        let mut flights = Flights::default();
        let mut stream = vec![message(1, "a", "b"), message(2, "b", "a")];
        // What was said before the map opened does not fly.
        assert_eq!(flights.watch(&stream, 0, on_map, t0), 0);
        assert!(flights.is_empty());

        stream.push(message(3, "a", "b"));
        stream.push(message(4, "operator", "a"));
        stream.push(message(5, "a", "a"));
        assert_eq!(
            flights.watch(&stream, 0, on_map, t0),
            1,
            "only between two on the map"
        );
        let flight = flights.iter().next().unwrap().clone();
        assert_eq!(
            (flight.message, flight.from.as_str(), flight.to.as_str()),
            (3, "a", "b")
        );
        // Seen once: watching again, or an edit of it, flies nothing.
        stream[2].read = Some(1);
        assert_eq!(flights.watch(&stream, 0, on_map, t0), 0);

        // It eases from sender to recipient.
        assert_eq!(Flights::progress(&flight, t0), 0.0);
        let half = Flights::progress(&flight, t0 + FLIGHT / 2);
        assert!((half - 0.5).abs() < 1e-6);
        assert!(
            Flights::progress(&flight, t0 + FLIGHT / 4) < 0.25,
            "slow at first"
        );
        assert_eq!(Flights::progress(&flight, t0 + FLIGHT * 2), 1.0);

        flights.land(t0 + FLIGHT - Duration::from_millis(1));
        assert!(!flights.is_empty(), "still in the air");
        flights.land(t0 + FLIGHT);
        assert!(flights.is_empty(), "landed");
    }

    #[test]
    fn what_a_snapshot_brings_does_not_fly() {
        let t0 = Instant::now();
        let mut flights = Flights::default();
        // Opened before ainetd's snapshot: nothing to see yet.
        assert_eq!(flights.watch(&[], 0, |_| true, t0), 0);
        // The snapshot brings the history: none of it is new.
        let mut stream: Vec<Message> = (1..=5).map(|id| message(id, "a", "b")).collect();
        assert_eq!(flights.watch(&stream, 1, |_| true, t0), 0);
        assert!(flights.is_empty());
        stream.push(message(6, "b", "a"));
        assert_eq!(flights.watch(&stream, 1, |_| true, t0), 1, "what follows flies");
        // A reconnect's snapshot, with what was said meanwhile: still old.
        stream.push(message(7, "a", "b"));
        assert_eq!(flights.watch(&stream, 2, |_| true, t0), 0);
    }

    #[test]
    fn too_many_flights_drop_the_oldest() {
        let t0 = Instant::now();
        let mut flights = Flights::default();
        flights.watch(&[], 0, |_| true, t0);
        let stream: Vec<Message> = (1..=100).map(|id| message(id, "a", "b")).collect();
        flights.watch(&stream, 0, |_| true, t0);
        let ids: Vec<u64> = flights.iter().map(|f| f.message).collect();
        assert_eq!(ids.len(), MAX_FLIGHTS);
        assert_eq!(ids.last(), Some(&100));
    }

    #[test]
    fn the_camera_zooms_about_the_pointer_and_maps_back() {
        let area = Bounds::new(point(px(10.), px(20.)), size(px(400.), px(300.)));
        let mut camera = Camera::default();
        let p = layout::Point::new(100.0, 50.0);
        assert_eq!(camera.to_screen(area, p), point(px(110.), px(70.)));
        let at = point(px(150.), px(90.));
        let under = camera.to_layout(area, at);
        camera.zoom_at(area, at, 2.0);
        assert_eq!(camera.zoom, 2.0);
        let back = camera.to_screen(area, under);
        assert!((back.x - at.x).abs() < px(0.01) && (back.y - at.y).abs() < px(0.01));
        let round = camera.to_layout(area, camera.to_screen(area, p));
        assert!((round.x - p.x).abs() < 0.01 && (round.y - p.y).abs() < 0.01);
        // Within limits.
        camera.zoom_at(area, at, 100.0);
        assert_eq!(camera.zoom, MAX_ZOOM);
        let r = camera.rect(area, &layout::Rect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(r.size, size(px(10. * MAX_ZOOM), px(5. * MAX_ZOOM)));
    }

    #[cfg(not(target_os = "macos"))]
    mod headless {
        use gpui::{Modifiers, MouseUpEvent, TestAppContext, VisualTestContext};

        use super::*;
        use crate::tests::init_test;
        use crate::workspace::Workspace;

        fn demo(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<MapItem>, &mut VisualTestContext) {
            init_test(cx);
            let (w, cx) = cx.add_window_view(|window, cx| {
                let fleet = cx.new(FleetModel::demo);
                Workspace::new(fleet, None, window, cx)
            });
            cx.update(|window, _| window.activate_window());
            cx.run_until_parked();
            cx.simulate_keystrokes("ctrl-t");
            cx.run_until_parked();
            let map = w.read_with(cx, |w, cx| {
                w.active_pane()
                    .read(cx)
                    .active_item()
                    .and_then(|i| i.to_any().downcast::<MapItem>().ok())
                    .expect("the Map is the active tab")
            });
            (w, map, cx)
        }

        fn fleet(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<FleetModel> {
            w.read_with(cx, |w, _| w.fleet().clone())
        }

        fn by_seat(fleet: &Entity<FleetModel>, seat: &str, cx: &mut VisualTestContext) -> String {
            fleet.read_with(cx, |f, _| {
                f.store
                    .agents
                    .values()
                    .find(|a| a.seat.as_deref() == Some(seat))
                    .map(|a| a.id.clone())
                    .unwrap()
            })
        }

        fn node(id: &str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
            cx.debug_bounds(Box::leak(format!("MAP-NODE-{id}").into_boxed_str()))
                .unwrap_or_else(|| panic!("{id} is drawn"))
        }

        #[gpui::test]
        async fn ctrl_t_draws_every_live_agent_inside_the_map(cx: &mut TestAppContext) {
            let (w, map, cx) = demo(cx);
            let fleet = fleet(&w, cx);
            assert!(cx.update(|window, cx| map.focus_handle(cx).is_focused(window)));
            let (area, layout) = map.read_with(cx, |m, _| m.drawn().cloned().expect("drawn"));
            let live: Vec<String> = fleet.read_with(cx, |f, _| {
                f.store
                    .agents
                    .values()
                    .filter(|a| a.state.is_live())
                    .map(|a| a.id.clone())
                    .collect()
            });
            assert_eq!(layout.nodes.len(), live.len());
            assert_eq!(layout.groups.len(), 2, "docs and infra");
            let drawn: Vec<Bounds<Pixels>> = live.iter().map(|id| node(id, cx)).collect();
            for (i, b) in drawn.iter().enumerate() {
                assert!(
                    area.contains(&b.origin) && area.contains(&b.bottom_right()),
                    "{b:?} in {area:?}"
                );
                for other in &drawn[i + 1..] {
                    assert!(!b.intersects(other), "{b:?} and {other:?}");
                }
            }
            // The lead's three edges, under it.
            let lead = by_seat(&fleet, "orch-lead@docs", cx);
            let ui = by_seat(&fleet, "dev-ui@docs", cx);
            assert_eq!(layout.edges.len(), 3);
            assert!(node(&ui, cx).top() > node(&lead, cx).bottom());

            // Zooming in and fitting again.
            let before = node(&lead, cx);
            cx.simulate_keystrokes("ctrl-=");
            assert_eq!(map.read_with(cx, |m, _| m.camera().zoom), ZOOM_STEP);
            cx.run_until_parked();
            let after = node(&lead, cx);
            assert!((after.size.width - before.size.width * ZOOM_STEP).abs() < px(0.5));
            cx.simulate_keystrokes("ctrl-0");
            assert_eq!(map.read_with(cx, |m, _| m.camera()), Camera::default());
        }

        #[gpui::test]
        async fn hovering_lights_a_node_a_click_selects_and_a_double_click_opens(cx: &mut TestAppContext) {
            let (w, map, cx) = demo(cx);
            let fleet = fleet(&w, cx);
            let app = by_seat(&fleet, "dev-app@docs", cx);
            assert_ne!(
                fleet.read_with(cx, |f, _| f.store.selected.clone()),
                Some(app.clone())
            );
            let at = node(&app, cx).center();

            cx.simulate_mouse_move(at, None, Modifiers::none());
            assert_eq!(
                map.read_with(cx, |m, _| m.hovered().map(str::to_string)),
                Some(app.clone())
            );
            cx.simulate_click(at, Modifiers::none());
            assert_eq!(
                fleet.read_with(cx, |f, _| f.store.selected.clone()),
                Some(app.clone())
            );

            // A double click opens its terminal as a tab.
            let pane = w.read_with(cx, |w, _| w.active_pane().clone());
            let tabs = |cx: &mut VisualTestContext| pane.read_with(cx, |p, _| p.items().count());
            let before = tabs(cx);
            for count in [1, 2] {
                cx.simulate_event(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: count,
                    first_mouse: false,
                });
                cx.simulate_event(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: count,
                });
            }
            assert_eq!(tabs(cx), before + 1);
            let name = fleet.read_with(cx, |f, _| f.store.agents[&app].name.clone());
            let title = pane.read_with(cx, |p, cx| p.active_item().unwrap().tab_content_text(cx));
            assert_eq!(title.as_ref(), name);
        }

        #[gpui::test]
        async fn dragging_the_background_pans(cx: &mut TestAppContext) {
            let (_, map, cx) = demo(cx);
            let (area, _) = map.read_with(cx, |m, _| m.drawn().cloned().unwrap());
            // A corner of the map, outside every cluster's nodes.
            let from = point(area.left() + px(2.), area.bottom() - px(2.));
            let to = point(from.x + px(30.), from.y - px(10.));
            cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
            assert_eq!(map.read_with(cx, |m, _| m.camera().pan), point(px(30.), px(-10.)));
        }

        #[gpui::test]
        async fn a_new_message_flies_from_sender_to_recipient(cx: &mut TestAppContext) {
            let (w, map, cx) = demo(cx);
            let fleet = fleet(&w, cx);
            assert!(
                map.read_with(cx, |m, _| m.flights().is_empty()),
                "old messages stay put"
            );
            let (ui, app) = (
                by_seat(&fleet, "dev-ui@docs", cx),
                by_seat(&fleet, "dev-app@docs", cx),
            );
            let next = fleet.read_with(cx, |f, _| {
                f.store.messages.iter().map(|m| m.id).max().unwrap() + 1
            });
            fleet.update(cx, |f, cx| {
                let message = message(next, &ui, &app);
                f.receive(vec![Msg::Event(Box::new(Event::Message { message }))], cx)
            });
            cx.run_until_parked();
            let flying: Vec<(String, String)> = map.read_with(cx, |m, _| {
                m.flights()
                    .iter()
                    .map(|f| (f.from.clone(), f.to.clone()))
                    .collect()
            });
            assert_eq!(flying, [(ui.clone(), app.clone())]);

            // Still in the air half way, landed after a second (the demo
            // may have sent others meanwhile).
            let ours = |cx: &mut VisualTestContext| {
                map.read_with(cx, |m, _| m.flights().iter().any(|f| f.message == next))
            };
            cx.executor().advance_clock(FLIGHT / 2);
            map.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            assert!(ours(cx));
            cx.executor().advance_clock(FLIGHT);
            map.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
            assert!(!ours(cx));
        }

        #[gpui::test]
        async fn an_empty_fleet_says_so(cx: &mut TestAppContext) {
            init_test(cx);
            let (map, cx) = cx.add_window_view(|_, cx| {
                let fleet = cx.new(|_| FleetModel::offline());
                MapItem::new(fleet, cx)
            });
            cx.run_until_parked();
            let (_, layout) = map.read_with(cx, |m, _| m.drawn().cloned().unwrap());
            assert!(layout.nodes.is_empty());
        }

        /// ainetd's snapshot arrives after the map opened (`--view map`,
        /// or opening it while connecting): its history stays put.
        #[gpui::test]
        async fn the_history_a_late_snapshot_brings_does_not_fly(cx: &mut TestAppContext) {
            init_test(cx);
            let fleet = cx.new(|_| FleetModel::offline());
            let (map, cx) = cx.add_window_view({
                let fleet = fleet.clone();
                move |_, cx| MapItem::new(fleet, cx)
            });
            cx.run_until_parked();
            let snapshot = Msg::Snapshot(Box::new(ainet_model::Demo::new().snapshot()));
            fleet.update(cx, |f, cx| f.receive(vec![Msg::Connected(true), snapshot], cx));
            cx.run_until_parked();
            let between_agents = fleet.read_with(cx, |f, _| {
                let live = |id: &str| f.store.agents.get(id).is_some_and(|a| a.state.is_live());
                f.store
                    .messages
                    .iter()
                    .filter(|m| m.from != m.to && live(&m.from) && live(&m.to))
                    .count()
            });
            assert!(between_agents > 0, "the history has messages to fly");
            assert!(map.read_with(cx, |m, _| m.flights().is_empty()));

            // What is said after it flies.
            let (from, to, next) = fleet.read_with(cx, |f, _| {
                let live = |id: &str| f.store.agents.get(id).is_some_and(|a| a.state.is_live());
                let m = f
                    .store
                    .messages
                    .iter()
                    .find(|m| m.from != m.to && live(&m.from) && live(&m.to));
                let m = m.unwrap();
                let next = f.store.messages.iter().map(|m| m.id).max().unwrap() + 1;
                (m.from.clone(), m.to.clone(), next)
            });
            fleet.update(cx, |f, cx| {
                let message = message(next, &from, &to);
                f.receive(vec![Msg::Event(Box::new(Event::Message { message }))], cx)
            });
            cx.run_until_parked();
            assert_eq!(map.read_with(cx, |m, _| m.flights().iter().count()), 1);
        }
    }
}
