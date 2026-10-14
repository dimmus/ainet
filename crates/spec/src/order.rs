//! Launch order: the seats sorted so that a delegator starts before the seats
//! it delegates to, and a parent before the seats it spawned.

use std::collections::BTreeSet;

use ainet_core::{Edge, EdgeKind};
use anyhow::bail;

use crate::ResolvedSeat;

/// The order to launch `seats` in, as indices into it.
///
/// `delegates_to` puts `from` before `to`; `spawned_by` puts `to` before
/// `from`; other edge kinds don't constrain anything. Seats otherwise keep
/// their order in `seats`. Edges name seats by logical id (or address);
/// edges to unknown seats are ignored. A cycle, including a seat delegating
/// to itself, is an error naming the seats in it.
pub fn launch_order(seats: &[ResolvedSeat], edges: &[Edge]) -> anyhow::Result<Vec<usize>> {
    let index = |id: &str| seats.iter().position(|s| s.logical == id || s.address == id);
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); seats.len()];
    let mut before: Vec<Vec<usize>> = vec![Vec::new(); seats.len()];
    for edge in edges {
        let (first, then) = match edge.kind {
            EdgeKind::DelegatesTo => (&edge.from, &edge.to),
            EdgeKind::SpawnedBy => (&edge.to, &edge.from),
            _ => continue,
        };
        if let (Some(a), Some(b)) = (index(first), index(then)) {
            after[a].push(b);
            before[b].push(a);
        }
    }

    let mut waiting: Vec<usize> = before.iter().map(Vec::len).collect();
    let mut ready: BTreeSet<usize> = (0..seats.len()).filter(|&i| waiting[i] == 0).collect();
    let mut order = Vec::with_capacity(seats.len());
    while let Some(next) = ready.pop_first() {
        order.push(next);
        for &b in &after[next] {
            waiting[b] -= 1;
            if waiting[b] == 0 {
                ready.insert(b);
            }
        }
    }
    if order.len() < seats.len() {
        let cycle = find_cycle(&waiting, &before);
        let names: Vec<&str> = cycle.iter().map(|&i| seats[i].logical.as_str()).collect();
        bail!(
            "delegates_to/spawned_by edges form a cycle: {} (each must launch before the next)",
            names.join(" -> ")
        );
    }
    Ok(order)
}

/// A cycle among the seats the sort could not place, in launch-before order,
/// first seat repeated at the end.
fn find_cycle(waiting: &[usize], before: &[Vec<usize>]) -> Vec<usize> {
    let stuck = |i: usize| waiting[i] > 0;
    let Some(start) = (0..waiting.len()).find(|&i| stuck(i)) else {
        return Vec::new();
    };
    // Every stuck seat waits on another stuck seat, so walking backwards
    // along what each waits for must come round to a seat already seen.
    let mut path = vec![start];
    let mut node = start;
    loop {
        let Some(&prev) = before[node].iter().find(|&&p| stuck(p)) else {
            return path;
        };
        if let Some(pos) = path.iter().position(|&p| p == prev) {
            let mut cycle: Vec<usize> = path[pos..].to_vec();
            cycle.reverse();
            cycle.push(cycle[0]);
            return cycle;
        }
        path.push(prev);
        node = prev;
    }
}
