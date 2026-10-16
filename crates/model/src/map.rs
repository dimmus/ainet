//! The fleet as a map: each rig a cluster of its live agents, loose agents
//! in a cluster of their own, and who directs whom as edges (a rig's
//! `delegates_to` and `spawned_by`, and each agent's parent). Inside a
//! cluster the agents are laid out as the TUI's Map lays out the fleet:
//! in layers by depth in the tree, each layer spread across the width,
//! the layers spread down.
//!
//! [`layout`] places everything in a box of a given size; nothing
//! overlaps and nothing leaves the box. A front end draws the nodes at
//! [`Layout::scale`] times their natural size ([`NODE_W`] × [`NODE_H`]),
//! which is 1 unless the box is too small for them.

use std::collections::{HashMap, HashSet};

use ainet_core::{Agent, EdgeKind};

use crate::store::Store;

/// A node's natural size: a critter with its name, role and state below.
pub const NODE_W: f32 = 132.0;
pub const NODE_H: f32 = 88.0;
/// A cluster's title strip, above its nodes.
pub const HEADER: f32 = 28.0;
/// Room inside a cluster around its nodes.
pub const PAD: f32 = 12.0;
/// Room between clusters.
pub const GAP: f32 = 16.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

/// A rectangle: its top left corner and size, y growing down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    /// A `w` × `h` rectangle centred on `at`.
    pub fn around(at: Point, w: f32, h: f32) -> Self {
        Rect::new(at.x - w / 2.0, at.y - h / 2.0, w, h)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    /// Whether the two share more than an edge.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }

    /// Whether `other` lies wholly inside, give or take rounding.
    pub fn holds(&self, other: &Rect) -> bool {
        const E: f32 = 0.01;
        other.x >= self.x - E
            && other.y >= self.y - E
            && other.right() <= self.right() + E
            && other.bottom() <= self.bottom() + E
    }

    /// Smaller by `by` on every side.
    pub fn inset(&self, by: f32) -> Rect {
        Rect::new(self.x + by, self.y + by, self.w - 2.0 * by, self.h - 2.0 * by)
    }
}

/// A cluster: a rig, or the loose agents.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The rig's name; `None` for the loose agents.
    pub rig: Option<String>,
    pub bounds: Rect,
    /// Its nodes, as indexes into [`Layout::nodes`].
    pub nodes: Vec<usize>,
}

/// An agent on the map.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    /// Its cluster, as an index into [`Layout::groups`].
    pub group: usize,
    /// How far down its cluster's tree: 0 for those nobody there directs.
    pub depth: usize,
    pub bounds: Rect,
}

/// Who directs whom: `from` directs `to` (indexes into [`Layout::nodes`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// The rig's edge it comes from; `None` when only the agent's parent
    /// says so.
    pub kind: Option<EdgeKind>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub groups: Vec<Group>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// The nodes' size against their natural size: at most 1.
    pub scale: f32,
}

impl Layout {
    /// The node of agent `id`.
    pub fn node(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// The node under `p`, if any.
    pub fn node_at(&self, p: Point) -> Option<usize> {
        self.nodes.iter().position(|n| n.bounds.contains(p))
    }

    /// The edge between two nodes, either way round.
    pub fn edge_between(&self, a: usize, b: usize) -> Option<&Edge> {
        self.edges
            .iter()
            .find(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
    }

    /// Whether node `ix` is at either end of edge `e`.
    pub fn touches(&self, e: &Edge, ix: usize) -> bool {
        e.from == ix || e.to == ix
    }
}

/// Lays the fleet's live agents out in a `width` × `height` box.
pub fn layout(store: &Store, width: f32, height: f32) -> Layout {
    layout_where(store, |_| true, width, height)
}

/// [`layout`] of the live agents `keep` says yes to: one rig or pod of
/// it (the Topology view's Graph), the rest as if they were not there.
pub fn layout_where(store: &Store, keep: impl Fn(&Agent) -> bool, width: f32, height: f32) -> Layout {
    // The clusters: each rig with live agents, then the loose ones.
    let live = |a: &&Agent| a.state.is_live() && keep(a);
    let mut members: Vec<(Option<String>, Vec<&Agent>)> = Vec::new();
    for name in store.rigs.keys() {
        let agents: Vec<&Agent> = store
            .agents
            .values()
            .filter(live)
            .filter(|a| a.rig.as_deref() == Some(name))
            .collect();
        if !agents.is_empty() {
            members.push((Some(name.clone()), agents));
        }
    }
    let loose: Vec<&Agent> = store
        .agents
        .values()
        .filter(live)
        .filter(|a| a.rig.as_ref().is_none_or(|r| !store.rigs.contains_key(r)))
        .collect();
    if !loose.is_empty() {
        members.push((None, loose));
    }
    if members.is_empty() {
        return Layout {
            scale: 1.0,
            ..Layout::default()
        };
    }

    let group_of: HashMap<&str, usize> = members
        .iter()
        .enumerate()
        .flat_map(|(g, (_, agents))| agents.iter().map(move |a| (a.id.as_str(), g)))
        .collect();
    let ties = ties(store, &group_of);

    // Each cluster's layers, the tree's roots first.
    let layers: Vec<Vec<Vec<&Agent>>> = members
        .iter()
        .enumerate()
        .map(|(g, (_, agents))| layers(agents, g, &group_of, &ties))
        .collect();
    // What each cluster needs at natural size, with the gap around it.
    let needs: Vec<(f32, f32)> = layers
        .iter()
        .map(|l| {
            let cols = l.iter().map(Vec::len).max().unwrap_or(1).max(1) as f32;
            let rows = l.len().max(1) as f32;
            (
                cols * NODE_W + 2.0 * PAD + GAP,
                HEADER + rows * NODE_H + PAD + GAP,
            )
        })
        .collect();

    let (width, height) = (width.max(1.0), height.max(1.0));
    let (cells, fit) = pack(&needs, width, height);
    let scale = fit.min(1.0);

    let mut layout = Layout {
        scale,
        ..Layout::default()
    };
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (g, ((rig, _), cell)) in members.iter().zip(&cells).enumerate() {
        let bounds = cell.inset(GAP * scale / 2.0);
        let content = Rect::new(
            bounds.x + PAD * scale,
            bounds.y + HEADER * scale,
            bounds.w - 2.0 * PAD * scale,
            bounds.h - (HEADER + PAD) * scale,
        );
        let mut nodes = Vec::new();
        let rows = layers[g].len() as f32;
        for (d, layer) in layers[g].iter().enumerate() {
            let y = content.y + (d as f32 + 0.5) * content.h / rows;
            let n = layer.len() as f32;
            for (i, a) in layer.iter().enumerate() {
                let x = content.x + (i as f32 + 0.5) * content.w / n;
                index.insert(&a.id, layout.nodes.len());
                nodes.push(layout.nodes.len());
                layout.nodes.push(Node {
                    id: a.id.clone(),
                    group: g,
                    depth: d,
                    bounds: Rect::around(Point::new(x, y), NODE_W * scale, NODE_H * scale),
                });
            }
        }
        layout.groups.push(Group {
            rig: rig.clone(),
            bounds,
            nodes,
        });
    }
    layout.edges = ties
        .iter()
        .filter_map(|(from, to, kind)| {
            Some(Edge {
                from: *index.get(from.as_str())?,
                to: *index.get(to.as_str())?,
                kind: *kind,
            })
        })
        .collect();
    layout
}

/// Who directs whom among the agents on the map, by id: each rig's
/// `delegates_to` (from directs to) and `spawned_by` (to directs from)
/// edges between seats with agents in them, then each agent's parent.
/// Once per pair, the rig's word first.
fn ties(store: &Store, on_map: &HashMap<&str, usize>) -> Vec<(String, String, Option<EdgeKind>)> {
    let mut out: Vec<(String, String, Option<EdgeKind>)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut add = |from: &str, to: &str, kind: Option<EdgeKind>, out: &mut Vec<_>| {
        if from != to
            && on_map.contains_key(from)
            && on_map.contains_key(to)
            && seen.insert((from.to_string(), to.to_string()))
        {
            out.push((from.to_string(), to.to_string(), kind));
        }
    };
    for rig in store.rigs.values() {
        let agent = |logical: &str| rig.seat(logical).and_then(|s| s.agent.clone());
        for e in &rig.edges {
            let (from, to) = match e.kind {
                EdgeKind::DelegatesTo => (&e.from, &e.to),
                EdgeKind::SpawnedBy => (&e.to, &e.from),
                _ => continue,
            };
            if let (Some(from), Some(to)) = (agent(from), agent(to)) {
                add(&from, &to, Some(e.kind), &mut out);
            }
        }
    }
    for a in store.agents.values() {
        if let Some(parent) = &a.parent {
            add(parent, &a.id, None, &mut out);
        }
    }
    out
}

/// A cluster's agents in layers by depth: an agent sits one layer below
/// the first agent of its cluster that directs it. Roots in seat order,
/// then each layer under its directors, left to right.
fn layers<'a>(
    agents: &[&'a Agent],
    group: usize,
    group_of: &HashMap<&str, usize>,
    ties: &[(String, String, Option<EdgeKind>)],
) -> Vec<Vec<&'a Agent>> {
    let parent: HashMap<&str, &str> = agents
        .iter()
        .filter_map(|a| {
            let (from, _, _) = ties
                .iter()
                .find(|(from, to, _)| to == &a.id && group_of.get(from.as_str()) == Some(&group))?;
            Some((a.id.as_str(), from.as_str()))
        })
        .collect();
    let depth = |id: &str| {
        let mut seen = HashSet::from([id]);
        let mut d = 0;
        let mut cur = id;
        while let Some(&p) = parent.get(cur) {
            // A loop in the edges: stop where it closes.
            if !seen.insert(p) {
                break;
            }
            d += 1;
            cur = p;
        }
        d
    };
    let depths: HashMap<&str, usize> = agents.iter().map(|a| (a.id.as_str(), depth(&a.id))).collect();
    let deepest = depths.values().copied().max().unwrap_or(0);
    let mut out: Vec<Vec<&Agent>> = Vec::new();
    for d in 0..=deepest {
        let above: HashMap<&str, usize> = out
            .last()
            .map(|l: &Vec<&Agent>| l.iter().enumerate().map(|(i, a)| (a.id.as_str(), i)).collect())
            .unwrap_or_default();
        let mut layer: Vec<&Agent> = agents
            .iter()
            .copied()
            .filter(|a| depths[a.id.as_str()] == d)
            .collect();
        layer.sort_by_key(|a| {
            let under = parent
                .get(a.id.as_str())
                .and_then(|p| above.get(p))
                .copied()
                .unwrap_or(usize::MAX);
            (under, a.seat.clone(), a.started_at, a.id.clone())
        });
        if !layer.is_empty() {
            out.push(layer);
        }
    }
    out
}

/// Packs clusters needing `needs` (width, height) into rows across a
/// `width` × `height` box, in order: each row as tall as its tallest
/// cluster needs, each cluster as wide as it needs, all grown or shrunk
/// alike to fill the box. Tries every number of columns and keeps the one
/// that lets the clusters grow most. Returns each cluster's cell and how
/// much it grew (below 1: shrank).
fn pack(needs: &[(f32, f32)], width: f32, height: f32) -> (Vec<Rect>, f32) {
    let mut best: Option<(Vec<Rect>, f32)> = None;
    for cols in 1..=needs.len() {
        let rows: Vec<&[(f32, f32)]> = needs.chunks(cols).collect();
        let total_h: f32 = rows
            .iter()
            .map(|r| r.iter().map(|n| n.1).fold(0.0, f32::max))
            .sum();
        let mut cells = Vec::with_capacity(needs.len());
        let mut fit = f32::INFINITY;
        let mut y = 0.0;
        for row in &rows {
            let row_w: f32 = row.iter().map(|n| n.0).sum();
            let row_h = row.iter().map(|n| n.1).fold(0.0, f32::max);
            let h = height * row_h / total_h;
            let mut x = 0.0;
            for need in row.iter() {
                let w = width * need.0 / row_w;
                fit = fit.min(w / need.0).min(h / need.1);
                cells.push(Rect::new(x, y, w, h));
                x += w;
            }
            y += h;
        }
        if best.as_ref().is_none_or(|(_, f)| fit > *f) {
            best = Some((cells, fit));
        }
    }
    best.unwrap_or_default()
}

/// Where an edge from `from` to `to` runs, as a cubic Bézier curve (start,
/// two control points, end): from under the director to over the one it
/// directs when that one sits lower, the other way up when higher, and
/// side to side when they sit level.
pub fn route(from: &Rect, to: &Rect) -> [Point; 4] {
    let (a, b) = (from.center(), to.center());
    if b.y - a.y > from.h / 2.0 {
        let (s, e) = (Point::new(a.x, from.bottom()), Point::new(b.x, to.y));
        let d = (e.y - s.y) / 2.0;
        [s, Point::new(s.x, s.y + d), Point::new(e.x, e.y - d), e]
    } else if a.y - b.y > from.h / 2.0 {
        let (s, e) = (Point::new(a.x, from.y), Point::new(b.x, to.bottom()));
        let d = (s.y - e.y) / 2.0;
        [s, Point::new(s.x, s.y - d), Point::new(e.x, e.y + d), e]
    } else {
        let right = b.x >= a.x;
        let (s, e) = if right {
            (Point::new(from.right(), a.y), Point::new(to.x, b.y))
        } else {
            (Point::new(from.x, a.y), Point::new(to.right(), b.y))
        };
        let d = (e.x - s.x) / 2.0;
        [s, Point::new(s.x + d, s.y), Point::new(e.x - d, e.y), e]
    }
}

/// The point `t` (0 to 1) of the way along a cubic Bézier curve.
pub fn bezier(c: &[Point; 4], t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        a * c[0].x + b * c[1].x + cc * c[2].x + d * c[3].x,
        a * c[0].y + b * c[1].y + cc * c[2].y + d * c[3].y,
    )
}

/// The TUI's work in flight along an edge: three blobs `t` of the way
/// from the director to the one it directs at animation tick `tick`,
/// spaced apart and offset per agent (`id`) so edges do not pulse in step.
/// `upward` runs them the other way (a question going up).
pub fn pulses(tick: u64, id: &str, upward: bool) -> [f64; 3] {
    let phase = (tick + u64::from(ainet_critter::hash(id)) % 40) % 40;
    [0u64, 1, 2].map(|k| {
        let t = ((phase + k * 13) % 40) as f64 / 40.0;
        if upward { 1.0 - t } else { t }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ainet_core::{AgentState, Edge as RigEdge, Rig, RigStatus, Runtime, Seat, SeatStatus, Transport};

    use super::*;
    use crate::demo::Demo;
    use crate::link::Msg;

    fn demo() -> Store {
        let mut store = Store::new();
        store.apply(Msg::Snapshot(Box::new(Demo::new().snapshot())));
        store
    }

    fn by_seat(store: &Store, seat: &str) -> String {
        store
            .agents
            .values()
            .find(|a| a.seat.as_deref() == Some(seat))
            .map(|a| a.id.clone())
            .unwrap_or_else(|| panic!("no agent in {seat}"))
    }

    fn agent(id: &str, rig: Option<&str>, parent: Option<&str>) -> Agent {
        Agent {
            id: id.into(),
            name: id.into(),
            runtime: Runtime::ClaudeCode,
            role: None,
            task: None,
            model: None,
            cwd: PathBuf::new(),
            branch: None,
            state: AgentState::Idle,
            state_since: 0,
            started_at: 0,
            context: None,
            parent: parent.map(Into::into),
            pid: None,
            last_tool: None,
            activity: Vec::new(),
            seat: rig.map(|r| format!("p-{id}@{r}")),
            rig: rig.map(Into::into),
            transcript: None,
            unread: 0,
            writable: true,
            terminal: true,
        }
    }

    fn assert_sound(layout: &Layout, w: f32, h: f32) {
        let room = Rect::new(0.0, 0.0, w, h);
        for (i, g) in layout.groups.iter().enumerate() {
            assert!(
                room.holds(&g.bounds),
                "cluster {i} leaves the box: {:?}",
                g.bounds
            );
            for other in &layout.groups[i + 1..] {
                assert!(!g.bounds.overlaps(&other.bounds), "clusters overlap");
            }
        }
        for (i, n) in layout.nodes.iter().enumerate() {
            assert!(
                layout.groups[n.group].bounds.holds(&n.bounds),
                "{} leaves its cluster at {w}x{h}",
                n.id
            );
            for other in &layout.nodes[i + 1..] {
                assert!(
                    !n.bounds.inset(0.01).overlaps(&other.bounds.inset(0.01)),
                    "{} and {} overlap at {w}x{h}",
                    n.id,
                    other.id
                );
            }
        }
        assert!(layout.scale > 0.0 && layout.scale <= 1.0);
    }

    #[test]
    fn the_demo_lays_out_as_two_rigs_without_overlaps() {
        let store = demo();
        let layout = layout(&store, 1200.0, 700.0);
        let rigs: Vec<Option<&str>> = layout.groups.iter().map(|g| g.rig.as_deref()).collect();
        // The loose researcher is gone, so there is no loose cluster.
        assert_eq!(rigs, [Some("docs"), Some("infra")]);
        assert_eq!(layout.nodes.len(), 7, "every live agent");
        assert_eq!(layout.scale, 1.0, "room enough");
        assert_sound(&layout, 1200.0, 700.0);

        // The lead above the three it directs; the reviewer beside it.
        let node = |seat: &str| &layout.nodes[layout.node(&by_seat(&store, seat)).unwrap()];
        let lead = node("orch-lead@docs");
        assert_eq!(lead.depth, 0);
        assert_eq!(node("rev-review@docs").depth, 0);
        for dev in ["dev-ui@docs", "dev-app@docs", "dev-writer@docs"] {
            assert_eq!(node(dev).depth, 1, "{dev}");
            assert!(node(dev).bounds.y > lead.bounds.bottom(), "{dev} under the lead");
        }
        // The docs rig, three wide and two deep, gets more room than infra.
        let area = |g: &Group| g.bounds.w * g.bounds.h;
        assert!(area(&layout.groups[0]) > area(&layout.groups[1]));
        for seat in ["ops-data@infra", "ops-sre@infra"] {
            assert_eq!(node(seat).group, 1);
        }
    }

    #[test]
    fn a_layout_can_hold_one_rig_or_pod() {
        let store = demo();
        let docs = layout_where(&store, |a| a.rig.as_deref() == Some("docs"), 900.0, 600.0);
        assert_eq!(docs.groups.len(), 1);
        assert_eq!(docs.groups[0].rig.as_deref(), Some("docs"));
        assert_eq!(docs.nodes.len(), 5);
        assert_eq!(docs.edges.len(), 3, "the lead's three");
        assert_sound(&docs, 900.0, 600.0);
        // Only the reviewer: no edges to agents left out.
        let rev = layout_where(
            &store,
            |a| a.seat.as_deref() == Some("rev-review@docs"),
            900.0,
            600.0,
        );
        assert_eq!((rev.nodes.len(), rev.edges.len()), (1, 0));
        assert!(layout_where(&store, |_| false, 900.0, 600.0).nodes.is_empty());
    }

    #[test]
    fn the_demo_fits_any_box() {
        let store = demo();
        for (w, h) in [
            (1600.0, 900.0),
            (800.0, 600.0),
            (400.0, 900.0),
            (300.0, 200.0),
            (90.0, 60.0),
        ] {
            assert_sound(&layout(&store, w, h), w, h);
        }
        // A tall box stacks the rigs; a wide one puts them side by side.
        let tall = layout(&store, 500.0, 1400.0);
        assert!(tall.groups[1].bounds.y >= tall.groups[0].bounds.bottom());
        let wide = layout(&store, 1600.0, 500.0);
        assert!(wide.groups[1].bounds.x >= wide.groups[0].bounds.right());
        // Too small: the nodes shrink rather than overlap.
        assert!(super::layout(&store, 300.0, 200.0).scale < 1.0);
    }

    #[test]
    fn edges_are_the_rigs_and_the_parents_once_each() {
        let store = demo();
        let layout = layout(&store, 1200.0, 700.0);
        let lead = layout.node(&by_seat(&store, "orch-lead@docs")).unwrap();
        let mut directed: Vec<(usize, Option<EdgeKind>)> = layout
            .edges
            .iter()
            .inspect(|e| assert_eq!(e.from, lead, "only the lead directs"))
            .map(|e| (e.to, e.kind))
            .collect();
        directed.sort_by_key(|(to, _)| *to);
        let mut devs: Vec<usize> = ["dev-ui@docs", "dev-app@docs", "dev-writer@docs"]
            .iter()
            .map(|s| layout.node(&by_seat(&store, s)).unwrap())
            .collect();
        devs.sort();
        // `delegates_to` and the parent say the same: one edge each, and
        // `can_observe` is not who directs whom.
        assert_eq!(
            directed,
            devs.iter()
                .map(|d| (*d, Some(EdgeKind::DelegatesTo)))
                .collect::<Vec<_>>()
        );
        let ui = layout.node(&by_seat(&store, "dev-ui@docs")).unwrap();
        assert!(layout.edge_between(ui, lead).is_some(), "either way round");
        assert!(
            layout.edge_between(devs[0], devs[1]).is_none(),
            "peers are not tied"
        );
    }

    #[test]
    fn spawned_by_points_from_the_spawner_and_loose_agents_group() {
        let mut store = Store::new();
        let seat = |logical: &str, agent: &str| Seat {
            address: format!("{}@lab", logical.replace('.', "-")),
            logical: logical.into(),
            rig: "lab".into(),
            pod: "p".into(),
            member: logical.into(),
            label: None,
            role: None,
            runtime: Runtime::ClaudeCode,
            model: None,
            cwd: PathBuf::new(),
            agent: Some(agent.into()),
            status: SeatStatus::Running,
            transport: Transport::Pane,
            restore_policy: String::new(),
            typing_guard: false,
            held: 0,
        };
        store.rigs.insert(
            "lab".into(),
            Rig {
                name: "lab".into(),
                summary: None,
                spec_path: None,
                root: PathBuf::new(),
                status: RigStatus::Running,
                seats: vec![seat("p.boss", "boss"), seat("p.kid", "kid")],
                edges: vec![RigEdge {
                    kind: EdgeKind::SpawnedBy,
                    from: "p.kid".into(),
                    to: "p.boss".into(),
                }],
                created: 0,
            },
        );
        for a in [
            agent("boss", Some("lab"), None),
            agent("kid", Some("lab"), None),
            agent("solo", None, None),
            agent("helper", None, Some("solo")),
            // Directed from a rig into the loose cluster.
            agent("scout", None, Some("boss")),
        ] {
            store.agents.insert(a.id.clone(), a);
        }
        let layout = layout(&store, 900.0, 500.0);
        assert_sound(&layout, 900.0, 500.0);
        assert_eq!(layout.groups.len(), 2);
        assert_eq!(layout.groups[1].rig, None, "the loose agents last");
        let ix = |id: &str| layout.node(id).unwrap();
        let edge = |from: &str, to: &str| {
            layout
                .edges
                .iter()
                .find(|e| e.from == ix(from) && e.to == ix(to))
                .map(|e| e.kind)
        };
        assert_eq!(edge("boss", "kid"), Some(Some(EdgeKind::SpawnedBy)));
        assert_eq!(edge("solo", "helper"), Some(None));
        assert_eq!(edge("boss", "scout"), Some(None), "across clusters");
        assert_eq!(layout.nodes[ix("kid")].depth, 1);
        assert_eq!(layout.nodes[ix("helper")].depth, 1);
        // Directed from another cluster only: a root in its own.
        assert_eq!(layout.nodes[ix("scout")].depth, 0);
    }

    #[test]
    fn a_loop_in_the_edges_still_lays_out() {
        let mut store = Store::new();
        for a in [agent("a", None, Some("b")), agent("b", None, Some("a"))] {
            store.agents.insert(a.id.clone(), a);
        }
        let layout = layout(&store, 600.0, 400.0);
        assert_eq!(layout.nodes.len(), 2);
        assert_sound(&layout, 600.0, 400.0);
        assert!(super::layout(&Store::new(), 600.0, 400.0).nodes.is_empty());
    }

    #[test]
    fn routes_leave_and_enter_on_the_facing_sides() {
        let top = Rect::new(100.0, 0.0, 100.0, 80.0);
        let below = Rect::new(0.0, 200.0, 100.0, 80.0);
        let r = route(&top, &below);
        assert_eq!(r[0], Point::new(150.0, 80.0), "under the director");
        assert_eq!(r[3], Point::new(50.0, 200.0), "over the one it directs");
        assert_eq!(bezier(&r, 0.0), r[0]);
        assert_eq!(bezier(&r, 1.0), r[3]);
        let mid = bezier(&r, 0.5);
        assert!((mid.x - 100.0).abs() < 0.01 && (mid.y - 140.0).abs() < 0.01);
        // Upwards and level.
        let up = route(&below, &top);
        assert_eq!((up[0], up[3]), (Point::new(50.0, 200.0), Point::new(150.0, 80.0)));
        let beside = Rect::new(400.0, 10.0, 100.0, 80.0);
        let side = route(&top, &beside);
        assert_eq!(
            (side[0], side[3]),
            (Point::new(200.0, 40.0), Point::new(400.0, 50.0))
        );
        let back = route(&beside, &top);
        assert_eq!(
            (back[0], back[3]),
            (Point::new(400.0, 50.0), Point::new(200.0, 40.0))
        );
    }

    #[test]
    fn pulses_are_the_tuis() {
        let id = "c13f0e77";
        let phase = (7 + u64::from(ainet_critter::hash(id)) % 40) % 40;
        let down = pulses(7, id, false);
        for (k, t) in down.iter().enumerate() {
            assert_eq!(*t, ((phase + k as u64 * 13) % 40) as f64 / 40.0);
        }
        let up = pulses(7, id, true);
        assert_eq!(up, down.map(|t| 1.0 - t));
        // A full cycle every 40 ticks.
        assert_eq!(pulses(47, id, false), down);
    }
}
