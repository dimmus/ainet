//! Panes split right and down, after Zed's `workspace::pane_group`: a tree
//! of axes whose members are panes or further axes, each axis sharing its
//! length out by flexes that the dividers between members drag, as in Zed.
//! The tree is generic over the pane so its logic is tested without GPUI;
//! the workspace keeps a `PaneGroup<Entity<Pane>>`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, Axis, Bounds, Entity, IntoElement, ParentElement, Pixels, Point, div, px};
use ui::prelude::*;

use crate::pane::Pane;

/// How wide the grab area of a divider is.
pub const HANDLE_HITBOX_SIZE: f32 = 4.;
/// The smallest a pane can be dragged to, across and down, as in Zed.
const HORIZONTAL_MIN_SIZE: f32 = 80.;
const VERTICAL_MIN_SIZE: f32 = 100.;

/// Where a split puts the new pane, or where to look for a neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SplitDirection {
    pub fn axis(self) -> Axis {
        match self {
            SplitDirection::Up | SplitDirection::Down => Axis::Vertical,
            SplitDirection::Left | SplitDirection::Right => Axis::Horizontal,
        }
    }

    /// Whether the new pane comes after the old one.
    pub fn increasing(self) -> bool {
        matches!(self, SplitDirection::Down | SplitDirection::Right)
    }
}

/// Flexes and bounds shared with the element that draws an axis.
type Shared<T> = Rc<RefCell<Vec<T>>>;

#[derive(Clone, Debug)]
pub enum Member<P> {
    Axis(PaneAxis<P>),
    Pane(P),
}

#[derive(Clone, Debug)]
pub struct PaneAxis<P> {
    pub axis: Axis,
    pub members: Vec<Member<P>>,
    /// Each member's share of the axis; they add up to the member count.
    pub flexes: Shared<f32>,
    /// Where each member was last drawn, to find panes by position.
    pub bounding_boxes: Shared<Option<Bounds<Pixels>>>,
}

impl<P: Clone + PartialEq> PaneAxis<P> {
    pub fn new(axis: Axis, members: Vec<Member<P>>) -> Self {
        let n = members.len();
        PaneAxis {
            axis,
            members,
            flexes: Rc::new(RefCell::new(vec![1.; n])),
            bounding_boxes: Rc::new(RefCell::new(vec![None; n])),
        }
    }

    fn reset(&self) {
        *self.flexes.borrow_mut() = vec![1.; self.members.len()];
        *self.bounding_boxes.borrow_mut() = vec![None; self.members.len()];
    }

    fn split(&mut self, old: &P, new: &P, direction: SplitDirection) -> bool {
        for ix in 0..self.members.len() {
            match &mut self.members[ix] {
                Member::Axis(axis) => {
                    if axis.split(old, new, direction) {
                        return true;
                    }
                }
                Member::Pane(pane) if pane == old => {
                    if direction.axis() == self.axis {
                        let at = if direction.increasing() { ix + 1 } else { ix };
                        self.members.insert(at, Member::Pane(new.clone()));
                        self.reset();
                    } else {
                        self.members[ix] = Member::new_axis(old.clone(), new.clone(), direction);
                    }
                    return true;
                }
                Member::Pane(_) => {}
            }
        }
        false
    }

    /// Removes `pane`; returns whether it was found, and the one member
    /// left if the axis is down to one.
    fn remove(&mut self, pane: &P) -> (bool, Option<Member<P>>) {
        let mut found = false;
        for ix in 0..self.members.len() {
            match &mut self.members[ix] {
                Member::Axis(axis) => {
                    let (was, last) = axis.remove(pane);
                    if was {
                        if let Some(last) = last {
                            self.members[ix] = last;
                        }
                        found = true;
                        break;
                    }
                }
                Member::Pane(p) if p == pane => {
                    self.members.remove(ix);
                    self.reset();
                    found = true;
                    break;
                }
                Member::Pane(_) => {}
            }
        }
        if found && self.members.len() == 1 {
            return (true, self.members.pop());
        }
        (found, None)
    }

    fn bounding_box_for_pane(&self, pane: &P) -> Option<Bounds<Pixels>> {
        let boxes = self.bounding_boxes.borrow();
        self.members
            .iter()
            .enumerate()
            .find_map(|(ix, member)| match member {
                Member::Pane(p) if p == pane => boxes.get(ix).copied().flatten(),
                Member::Pane(_) => None,
                Member::Axis(axis) => axis.bounding_box_for_pane(pane),
            })
    }

    fn pane_at_pixel_position(&self, at: Point<Pixels>) -> Option<&P> {
        let boxes = self.bounding_boxes.borrow();
        for (ix, member) in self.members.iter().enumerate() {
            if boxes.get(ix).copied().flatten().is_some_and(|b| b.contains(&at)) {
                return match member {
                    Member::Pane(p) => Some(p),
                    Member::Axis(axis) => axis.pane_at_pixel_position(at),
                };
            }
        }
        None
    }
}

impl<P: Clone + PartialEq> Member<P> {
    fn new_axis(old: P, new: P, direction: SplitDirection) -> Self {
        let members = if direction.increasing() {
            vec![Member::Pane(old), Member::Pane(new)]
        } else {
            vec![Member::Pane(new), Member::Pane(old)]
        };
        Member::Axis(PaneAxis::new(direction.axis(), members))
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a P>) {
        match self {
            Member::Pane(p) => out.push(p),
            Member::Axis(axis) => axis.members.iter().for_each(|m| m.collect(out)),
        }
    }
}

/// The center's panes.
#[derive(Clone, Debug)]
pub struct PaneGroup<P> {
    pub root: Member<P>,
}

impl<P: Clone + PartialEq> PaneGroup<P> {
    pub fn new(pane: P) -> Self {
        PaneGroup {
            root: Member::Pane(pane),
        }
    }

    /// Puts `new` next to `old`, on the side `direction` says. If `old` is
    /// not in the group, splits the first pane.
    pub fn split(&mut self, old: &P, new: &P, direction: SplitDirection) {
        let found = match &mut self.root {
            Member::Pane(p) if p == old => {
                self.root = Member::new_axis(old.clone(), new.clone(), direction);
                true
            }
            Member::Pane(_) => false,
            Member::Axis(axis) => axis.split(old, new, direction),
        };
        if !found {
            let first = self.panes()[0].clone();
            self.split(&first, new, direction);
        }
    }

    /// Takes `pane` out, folding axes left with one member. The last pane
    /// stays: returns false for it, and for panes not in the group.
    pub fn remove(&mut self, pane: &P) -> bool {
        match &mut self.root {
            Member::Pane(_) => false,
            Member::Axis(axis) => {
                let (found, last) = axis.remove(pane);
                if let Some(last) = last {
                    self.root = last;
                }
                found
            }
        }
    }

    /// Every pane, left to right and top to bottom.
    pub fn panes(&self) -> Vec<&P> {
        let mut out = Vec::new();
        self.root.collect(&mut out);
        out
    }

    /// Where `pane` was last drawn (only known once split).
    pub fn bounding_box_for_pane(&self, pane: &P) -> Option<Bounds<Pixels>> {
        match &self.root {
            Member::Pane(_) => None,
            Member::Axis(axis) => axis.bounding_box_for_pane(pane),
        }
    }

    pub fn pane_at_pixel_position(&self, at: Point<Pixels>) -> Option<&P> {
        match &self.root {
            Member::Pane(p) => Some(p),
            Member::Axis(axis) => axis.pane_at_pixel_position(at),
        }
    }

    /// The pane just past `active`'s edge in `direction`, as Zed finds it:
    /// a point beyond the middle of that edge.
    pub fn find_pane_in_direction(&self, active: &P, direction: SplitDirection) -> Option<&P> {
        let b = self.bounding_box_for_pane(active)?;
        let center = b.center();
        let d = px(HANDLE_HITBOX_SIZE);
        let target = match direction {
            SplitDirection::Left => Point::new(b.left() - d, center.y),
            SplitDirection::Right => Point::new(b.right() + d, center.y),
            SplitDirection::Up => Point::new(center.x, b.top() - d),
            SplitDirection::Down => Point::new(center.x, b.bottom() + d),
        };
        self.pane_at_pixel_position(target)
    }
}

/// Moves the divider after member `ix` by `change` pixels of an axis
/// `length` long. Moving it forward grows member `ix`, taking the room
/// from the members after the divider in turn, each down to `min_size`;
/// moving it back grows the member after it from those before. The flexes
/// keep adding up to their count.
pub fn resize(flexes: &mut [f32], ix: usize, change: Pixels, length: Pixels, min_size: Pixels) {
    let n = flexes.len();
    if ix + 1 >= n || length <= Pixels::ZERO {
        return;
    }
    let unit = length / n as f32;
    let mut sizes: Vec<Pixels> = flexes.iter().map(|f| unit * *f).collect();
    let (grows, givers): (usize, Vec<usize>) = if change > Pixels::ZERO {
        (ix, (ix + 1..n).collect())
    } else {
        (ix + 1, (0..=ix).rev().collect())
    };
    let mut wanted = change.abs();
    for giver in givers {
        if wanted <= Pixels::ZERO {
            break;
        }
        let given = (sizes[giver] - min_size).max(Pixels::ZERO).min(wanted);
        sizes[giver] -= given;
        sizes[grows] += given;
        wanted -= given;
    }
    for (flex, size) in flexes.iter_mut().zip(sizes) {
        *flex = size / unit;
    }
}

impl PaneGroup<Entity<Pane>> {
    /// Draws the panes, split and resizable.
    pub fn render(&self) -> AnyElement {
        render_member(&self.root, 0)
    }
}

fn render_member(member: &Member<Entity<Pane>>, basis: usize) -> AnyElement {
    match member {
        Member::Pane(pane) => div()
            .relative()
            .flex_1()
            .size_full()
            .child(pane.clone())
            .into_any_element(),
        Member::Axis(axis) => {
            let children: Vec<AnyElement> = axis
                .members
                .iter()
                .enumerate()
                .map(|(ix, m)| render_member(m, (basis + ix + 1) * 10))
                .collect();
            element::pane_axis(axis.axis, basis, axis.flexes.clone(), axis.bounding_boxes.clone())
                .children(children)
                .into_any_element()
        }
    }
}

/// The element that lays out an axis by its flexes and drags its
/// dividers; Zed's `PaneAxisElement`, without the workspace's persistence.
mod element {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        Along, AnyElement, App, Axis, Bounds, CursorStyle, Element, GlobalElementId, Hitbox, HitboxBehavior,
        InspectorElementId, IntoElement, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
        Style, Window, px, relative, size,
    };
    use ui::prelude::*;

    use super::{HANDLE_HITBOX_SIZE, HORIZONTAL_MIN_SIZE, Shared, VERTICAL_MIN_SIZE, resize};

    const DIVIDER_SIZE: f32 = 1.;

    pub fn pane_axis(
        axis: Axis,
        basis: usize,
        flexes: Shared<f32>,
        bounding_boxes: Shared<Option<Bounds<Pixels>>>,
    ) -> PaneAxisElement {
        PaneAxisElement {
            axis,
            basis,
            flexes,
            bounding_boxes,
            children: Vec::new(),
        }
    }

    pub struct PaneAxisElement {
        axis: Axis,
        basis: usize,
        flexes: Shared<f32>,
        bounding_boxes: Shared<Option<Bounds<Pixels>>>,
        children: Vec<AnyElement>,
    }

    pub struct PaneAxisLayout {
        dragged_handle: Rc<RefCell<Option<usize>>>,
        children: Vec<ChildLayout>,
    }

    struct ChildLayout {
        bounds: Bounds<Pixels>,
        element: AnyElement,
        handle: Option<HandleLayout>,
    }

    struct HandleLayout {
        hitbox: Hitbox,
        divider_bounds: Bounds<Pixels>,
    }

    fn min_size(axis: Axis) -> Pixels {
        match axis {
            Axis::Horizontal => px(HORIZONTAL_MIN_SIZE),
            Axis::Vertical => px(VERTICAL_MIN_SIZE),
        }
    }

    impl IntoElement for PaneAxisElement {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    impl ParentElement for PaneAxisElement {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.children.extend(elements)
        }
    }

    impl Element for PaneAxisElement {
        type RequestLayoutState = ();
        type PrepaintState = PaneAxisLayout;

        fn id(&self) -> Option<ElementId> {
            Some(self.basis.into())
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (gpui::LayoutId, ()) {
            let style = Style {
                flex_grow: 1.,
                flex_shrink: 1.,
                flex_basis: relative(0.).into(),
                size: size(relative(1.).into(), relative(1.).into()),
                ..Style::default()
            };
            (window.request_layout(style, None, cx), ())
        }

        fn prepaint(
            &mut self,
            global_id: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) -> PaneAxisLayout {
            let dragged_handle = window.with_element_state::<Rc<RefCell<Option<usize>>>, _>(
                global_id.expect("the axis has an id"),
                |state, _| {
                    let state = state.unwrap_or_else(|| Rc::new(RefCell::new(None)));
                    (state.clone(), state)
                },
            );
            let flexes = self.flexes.borrow().clone();
            let len = self.children.len();
            let per_flex = bounds.size.along(self.axis) / len as f32;
            let mut origin = bounds.origin;
            let mut boxes = self.bounding_boxes.borrow_mut();
            boxes.clear();
            let mut layout = PaneAxisLayout {
                dragged_handle,
                children: Vec::with_capacity(len),
            };
            for (ix, mut child) in std::mem::take(&mut self.children).into_iter().enumerate() {
                let flex = flexes.get(ix).copied().unwrap_or(1.);
                let child_size = bounds
                    .size
                    .apply_along(self.axis, |_| per_flex * flex)
                    .map(|d| d.round());
                let child_bounds = Bounds {
                    origin,
                    size: child_size,
                };
                boxes.push(Some(child_bounds));
                child.layout_as_root(child_size.into(), window, cx);
                child.prepaint_at(origin, window, cx);
                origin = origin.apply_along(self.axis, |v| v + child_size.along(self.axis));
                layout.children.push(ChildLayout {
                    bounds: child_bounds,
                    element: child,
                    handle: None,
                });
            }
            for (ix, child) in layout.children.iter_mut().enumerate() {
                if ix + 1 < len {
                    child.handle = Some(layout_handle(self.axis, child.bounds, window));
                }
            }
            layout
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            _: &mut (),
            layout: &mut PaneAxisLayout,
            window: &mut Window,
            cx: &mut App,
        ) {
            for child in &mut layout.children {
                child.element.paint(window, cx);
            }
            for (ix, child) in layout.children.iter_mut().enumerate() {
                let Some(handle) = child.handle.as_mut() else {
                    continue;
                };
                let cursor = match self.axis {
                    Axis::Vertical => CursorStyle::ResizeRow,
                    Axis::Horizontal => CursorStyle::ResizeColumn,
                };
                if *layout.dragged_handle.borrow() == Some(ix) {
                    window.set_window_cursor_style(cursor);
                } else {
                    window.set_cursor_style(cursor, &handle.hitbox);
                }
                window.paint_quad(gpui::fill(
                    handle.divider_bounds,
                    cx.theme().colors().pane_group_border,
                ));
                window.on_mouse_event({
                    let dragged = layout.dragged_handle.clone();
                    let flexes = self.flexes.clone();
                    let hitbox = handle.hitbox.clone();
                    move |e: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble() && hitbox.is_hovered(window) {
                            dragged.replace(Some(ix));
                            // A double click evens the split out again.
                            if e.click_count >= 2 {
                                let mut flexes = flexes.borrow_mut();
                                *flexes = vec![1.; flexes.len()];
                                window.refresh();
                            }
                            cx.stop_propagation();
                        }
                    }
                });
                window.on_mouse_event({
                    let dragged = layout.dragged_handle.clone();
                    let flexes = self.flexes.clone();
                    let start = child.bounds.origin;
                    let axis = self.axis;
                    move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase.bubble() && *dragged.borrow() == Some(ix) {
                            let mut flexes = flexes.borrow_mut();
                            let length = bounds.size.along(axis);
                            let current = length * (flexes[ix] / flexes.len() as f32);
                            let change = (e.position - start).along(axis) - current;
                            resize(&mut flexes, ix, change, length, min_size(axis));
                            cx.stop_propagation();
                            window.refresh();
                        }
                    }
                });
            }
            window.on_mouse_event({
                let dragged = layout.dragged_handle.clone();
                move |_: &MouseUpEvent, phase, _, _| {
                    if phase.bubble() {
                        dragged.replace(None);
                    }
                }
            });
        }
    }

    fn layout_handle(axis: Axis, pane: Bounds<Pixels>, window: &mut Window) -> HandleLayout {
        let handle = Bounds {
            origin: pane
                .origin
                .apply_along(axis, |o| o + pane.size.along(axis) - px(HANDLE_HITBOX_SIZE / 2.)),
            size: pane.size.apply_along(axis, |_| px(HANDLE_HITBOX_SIZE)),
        };
        let divider_bounds = Bounds {
            origin: pane.origin.apply_along(axis, |o| o + pane.size.along(axis)),
            size: pane.size.apply_along(axis, |_| px(DIVIDER_SIZE)),
        };
        HandleLayout {
            hitbox: window.insert_hitbox(handle, HitboxBehavior::BlockMouse),
            divider_bounds,
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    fn boxes(axis: &PaneAxis<u32>, b: Vec<Bounds<Pixels>>) {
        *axis.bounding_boxes.borrow_mut() = b.into_iter().map(Some).collect();
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn splits_nest_and_removals_fold_back() {
        let mut g = PaneGroup::new(1);
        g.split(&1, &2, SplitDirection::Right);
        g.split(&2, &3, SplitDirection::Down);
        g.split(&1, &4, SplitDirection::Left);
        assert_eq!(g.panes(), [&4, &1, &2, &3]);
        let Member::Axis(root) = &g.root else { panic!() };
        assert_eq!(root.axis, Axis::Horizontal);
        assert_eq!(root.members.len(), 3, "a left split joins the row");
        assert_eq!(*root.flexes.borrow(), [1., 1., 1.]);

        // Splitting a pane that is not there splits the first.
        g.split(&99, &5, SplitDirection::Up);
        assert_eq!(g.panes(), [&5, &4, &1, &2, &3]);

        assert!(g.remove(&3));
        assert!(!g.remove(&3));
        assert!(g.remove(&5));
        assert!(g.remove(&4));
        assert_eq!(g.panes(), [&1, &2]);
        assert!(g.remove(&1));
        assert!(matches!(g.root, Member::Pane(2)));
        assert!(!g.remove(&2), "the last pane stays");
    }

    #[test]
    fn finds_neighbours_by_where_they_were_drawn() {
        // 1 | 2
        //   | -
        //   | 3
        let mut g = PaneGroup::new(1);
        g.split(&1, &2, SplitDirection::Right);
        g.split(&2, &3, SplitDirection::Down);
        let Member::Axis(root) = &g.root else { panic!() };
        boxes(
            root,
            vec![bounds(0., 0., 100., 200.), bounds(100., 0., 100., 200.)],
        );
        let Member::Axis(column) = &root.members[1] else {
            panic!()
        };
        boxes(
            column,
            vec![bounds(100., 0., 100., 150.), bounds(100., 150., 100., 50.)],
        );

        assert_eq!(g.find_pane_in_direction(&1, SplitDirection::Right), Some(&2));
        assert_eq!(g.find_pane_in_direction(&2, SplitDirection::Down), Some(&3));
        assert_eq!(g.find_pane_in_direction(&3, SplitDirection::Left), Some(&1));
        assert_eq!(g.find_pane_in_direction(&3, SplitDirection::Up), Some(&2));
        assert_eq!(g.find_pane_in_direction(&1, SplitDirection::Left), None);
        assert_eq!(g.bounding_box_for_pane(&3), Some(bounds(100., 150., 100., 50.)));
    }

    #[test]
    fn dragging_a_divider_keeps_the_minimum_and_the_total() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let total = |f: &[f32]| close(f.iter().sum(), 3.);
        // 300 px across: each 100. Drag the first divider 50 px right.
        let mut flexes = vec![1., 1., 1.];
        resize(&mut flexes, 0, px(50.), px(300.), px(40.));
        assert!(close(flexes[0], 1.5) && close(flexes[1], 0.5), "{flexes:?}");
        // The middle one stops at its minimum; the third gives the rest.
        resize(&mut flexes, 0, px(50.), px(300.), px(40.));
        assert!(
            close(flexes[0], 2.) && close(flexes[1], 0.4) && total(&flexes),
            "{flexes:?}"
        );
        // Back the other way, as far as the first one's minimum.
        resize(&mut flexes, 0, px(-500.), px(300.), px(40.));
        assert!(
            close(flexes[0], 0.4) && close(flexes[1], 2.) && total(&flexes),
            "{flexes:?}"
        );
        // There is no divider after the last member.
        let before = flexes.clone();
        resize(&mut flexes, 2, px(10.), px(300.), px(40.));
        assert_eq!(flexes, before);
    }
}
