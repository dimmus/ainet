//! The workspace saved as it is and restored at the next start, as Zed's
//! `workspace::persistence` does it: the file and its format are
//! `crate::persistence`; this is what the workspace reads into it and
//! builds back from it.
//!
//! Saving: the workspace is compared with what was written every
//! [`SAVE_CHECK`] and written once it has changed and then held still for
//! a check (the state lives in many cached views, so one comparison a
//! second is cheaper than hearing from all of them), and at once when the
//! window closes or the app quits.

use std::path::PathBuf;
use std::time::Duration;

use ainet_model::topology::Scope;
use gpui::{Axis, Entity, Focusable, Subscription, Task, point};
use ui::prelude::*;

use super::Workspace;
use crate::command_palette;
use crate::comms::CommsItem;
use crate::inbox::InboxItem;
use crate::item::{ItemHandle, Placeholder};
use crate::map::{Camera, MapItem};
use crate::pane::Pane;
use crate::pane_group::{Member, PaneAxis, PaneGroup};
use crate::persistence::{
    self, Debounce, SavedAxis, SavedDock, SavedItem, SavedMember, SavedWindow, SavedWorkspace,
};
use crate::terminal_item::TerminalItem;
use crate::terminals::TerminalsItem;
use crate::topology::{SubView, TopologyItem};

/// How often the workspace is compared with what was saved.
pub const SAVE_CHECK: Duration = Duration::from_millis(1000);

pub struct Persist {
    file: PathBuf,
    key: String,
    debounce: Debounce,
    /// The panes and the Fleet's selection saved, until ainetd's first
    /// snapshot says which agents are there to reopen terminals for and
    /// select; saved as they were meanwhile.
    pending: Option<Pending>,
    /// The window's bounds, as last seen.
    window: Option<SavedWindow>,
    _subscriptions: Vec<Subscription>,
    _check: Task<()>,
}

impl Workspace {
    /// Restores `saved` (what `file` holds under `key`) and saves this
    /// workspace there from now on. With `restore_center` false (a view
    /// was asked for on the command line) the panes are left as they are.
    pub fn persist(
        &mut self,
        file: PathBuf,
        key: String,
        saved: Option<SavedWorkspace>,
        restore_center: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(saved) = &saved {
            self.restore_layout(saved, window, cx);
        }
        let pending = saved.as_ref().filter(|_| restore_center).map(|s| Pending {
            center: s.center.clone(),
            selected: s.selected.clone(),
        });
        let subscriptions = vec![
            cx.observe_window_bounds(window, |this, window, _| {
                if let Some(persist) = &mut this.persist {
                    persist.window = Some(SavedWindow::from_bounds(window.window_bounds()));
                }
            }),
            cx.observe_in(&self.fleet, window, |this, _, window, cx| {
                this.restore_center_when_ready(window, cx)
            }),
        ];
        // Quitting, and the window going (however it goes), save first.
        cx.on_app_quit(|this, cx| {
            this.save_now(cx);
            async {}
        })
        .detach();
        cx.on_release(|this, cx| this.save_now(cx)).detach();
        let check = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SAVE_CHECK).await;
                let checked = this.update_in(cx, |this, window, cx| this.check_save(window, cx));
                if checked.is_err() {
                    break;
                }
            }
        });
        self.persist = Some(Persist {
            file,
            key,
            debounce: Debounce::new(saved),
            pending,
            window: Some(SavedWindow::from_bounds(window.window_bounds())),
            _subscriptions: subscriptions,
            _check: check,
        });
        self.restore_center_when_ready(window, cx);
    }

    /// The workspace as it would be saved now.
    pub fn to_saved(&self, cx: &App) -> Option<SavedWorkspace> {
        let persist = self.persist.as_ref()?;
        let docks = [&self.left_dock, &self.right_dock, &self.bottom_dock];
        let pending = persist.pending.as_ref();
        let center = match pending.and_then(|p| p.center.as_ref()) {
            Some(center) => Some(center.clone()),
            None => save_member(&self.center.root, &self.active_pane, cx)
                .retain(&|item| *item != SavedItem::Unknown),
        };
        let selected = match pending {
            Some(pending) => pending.selected.clone(),
            None => self.fleet.read(cx).store.selected.clone(),
        };
        Some(SavedWorkspace {
            window: persist.window,
            docks: docks
                .iter()
                .filter_map(|dock| {
                    let dock = dock.read(cx);
                    Some(SavedDock {
                        panel: dock.active_panel_name()?.to_string(),
                        open: dock.is_open(),
                    })
                })
                .collect(),
            panel_sizes: docks
                .iter()
                .flat_map(|dock| dock.read(cx).panel_sizes())
                .filter_map(|(name, size)| Some((name.to_string(), size?.as_f32())))
                .collect(),
            center,
            folded_rigs: self.fleet_panel.read(cx).folded_rigs(),
            selected,
            recent_actions: command_palette::recent(cx),
            command_history: self.command_line.read(cx).history().to_vec(),
        })
    }

    /// Writes the workspace now, unless it is what was written last.
    pub fn save_now(&mut self, cx: &App) {
        let Some(now) = self.to_saved(cx) else { return };
        if let Some(persist) = &mut self.persist
            && !persist.debounce.is_written(&now)
        {
            persist.write(now);
        }
    }

    /// Writes the workspace if it changed and has held still since the
    /// last check.
    fn check_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(persist) = &mut self.persist {
            persist.window = Some(SavedWindow::from_bounds(window.window_bounds()));
        }
        let Some(now) = self.to_saved(cx) else { return };
        if let Some(persist) = &mut self.persist
            && let Some(now) = persist.debounce.check(now)
        {
            persist.write(now);
        }
    }

    /// The docks, their panels' sizes, the Fleet's folds, the palette's
    /// recent actions and the command line's history, as saved.
    fn restore_layout(&mut self, saved: &SavedWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        let docks = [
            self.left_dock.clone(),
            self.right_dock.clone(),
            self.bottom_dock.clone(),
        ];
        for SavedDock { panel, open } in &saved.docks {
            // By panel: the Fleet may have moved sides since.
            if let Some((dock, ix)) = docks
                .iter()
                .find_map(|d| Some((d, d.read(cx).panel_index(panel)?)))
            {
                dock.update(cx, |dock, cx| {
                    dock.activate_panel(ix, window, cx);
                    dock.set_open(*open, window, cx);
                });
            }
        }
        for (name, size) in &saved.panel_sizes {
            if size.is_finite() {
                let size = px(*size).max(super::MIN_DOCK_SIZE);
                for dock in &docks {
                    dock.update(cx, |dock, cx| dock.set_panel_size(name, size, cx));
                }
            }
        }
        let folded = saved.folded_rigs.clone();
        self.fleet_panel
            .update(cx, |panel, cx| panel.set_folded_rigs(folded, cx));
        command_palette::set_recent(&saved.recent_actions, cx);
        let history = saved.command_history.clone();
        self.command_line.update(cx, |line, _| line.set_history(history));
        self.release_hidden_focus(window, cx);
        cx.notify();
    }

    /// Reopens the saved panes and selects the saved agent once the fleet
    /// knows its agents (ainetd's first snapshot; the demo has its own at
    /// once), without the tabs of agents that are gone. Tabs opened
    /// meanwhile win: then the saved panes and selection are dropped.
    fn restore_center_when_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.fleet.read(cx).snapshots() > 0;
        let Some(pending) = self
            .persist
            .as_mut()
            .filter(|_| ready)
            .and_then(|p| p.pending.take())
        else {
            return;
        };
        if !self.center_is_untouched(cx) {
            return;
        }
        if let Some(saved) = pending.center {
            self.restore_center(saved, window, cx);
        }
        if let Some(id) = pending.selected {
            self.fleet.update(cx, |fleet, cx| {
                if fleet.store.agents.get(&id).is_some_and(|a| a.state.is_live()) {
                    fleet.select(&id, cx);
                }
            });
        }
    }

    /// Replaces the untouched center with the saved panes, less the tabs
    /// of agents gone. The keyboard goes to the restored active tab if it
    /// was in the center, as Zed does; at start it stays in the Fleet.
    fn restore_center(&mut self, saved: SavedMember, window: &mut Window, cx: &mut Context<Self>) {
        let saved = {
            let store = &self.fleet.read(cx).store;
            saved.retain(&|item| match item {
                SavedItem::Terminal { agent } => store.agents.get(agent).is_some_and(|a| a.state.is_live()),
                SavedItem::Unknown => false,
                SavedItem::Inbox
                | SavedItem::Comms
                | SavedItem::Map { .. }
                | SavedItem::Terminals { .. }
                | SavedItem::Topology { .. } => true,
            })
        };
        let Some(saved) = saved else { return };
        let had_focus = window.focused(cx).is_none()
            || self
                .center
                .panes()
                .iter()
                .any(|p| p.read(cx).has_focus(window, cx));
        let mut active = None;
        let root = self.build_member(saved, &mut active, window, cx);
        let old = std::mem::replace(&mut self.center, PaneGroup { root });
        self.active_pane = active.unwrap_or_else(|| self.center.panes()[0].clone());
        for pane in old.panes() {
            pane.update(cx, |pane, cx| pane.close_all_items(window, cx));
        }
        if had_focus {
            // The pane hands the keyboard on to its active tab.
            window.focus(&self.active_pane.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Whether the center is as a new window has it: one pane, with at
    /// most the stand-in tab.
    fn center_is_untouched(&self, cx: &App) -> bool {
        let panes = self.center.panes();
        panes.len() == 1
            && panes[0]
                .read(cx)
                .items()
                .all(|item| item.to_any().downcast::<Placeholder>().is_ok())
    }

    fn build_member(
        &mut self,
        saved: SavedMember,
        active: &mut Option<Entity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Member<Entity<Pane>> {
        match saved {
            SavedMember::Pane {
                items,
                active: active_item,
                active_pane,
            } => {
                let pane = self.add_pane(window, cx);
                let items: Vec<Box<dyn ItemHandle>> = items
                    .into_iter()
                    .filter_map(|item| self.restore_item(item, window, cx))
                    .collect();
                pane.update(cx, |pane, cx| pane.restore_items(items, active_item, window, cx));
                if active_pane {
                    *active = Some(pane.clone());
                }
                Member::Pane(pane)
            }
            SavedMember::Axis {
                axis,
                members,
                flexes,
            } => {
                let members: Vec<_> = members
                    .into_iter()
                    .map(|m| self.build_member(m, active, window, cx))
                    .collect();
                let axis = match axis {
                    SavedAxis::Horizontal => Axis::Horizontal,
                    SavedAxis::Vertical => Axis::Vertical,
                };
                let axis = PaneAxis::new(axis, members);
                let n = axis.members.len();
                *axis.flexes.borrow_mut() = persistence::flexes_for(&flexes, n);
                Member::Axis(axis)
            }
        }
    }

    fn restore_item(
        &self,
        saved: SavedItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn ItemHandle>> {
        let fleet = self.fleet.clone();
        Some(match saved {
            SavedItem::Terminal { agent } => {
                Box::new(cx.new(|cx| TerminalItem::new(fleet, agent, window, cx)))
            }
            SavedItem::Inbox => Box::new(cx.new(|cx| InboxItem::new(fleet, window, cx))),
            SavedItem::Comms => Box::new(cx.new(|cx| CommsItem::new(fleet, window, cx))),
            SavedItem::Map { zoom, pan_x, pan_y } => {
                let map = cx.new(|cx| MapItem::new(fleet, cx));
                let camera = Camera {
                    zoom,
                    pan: point(px(pan_x), px(pan_y)),
                };
                map.update(cx, |map, cx| map.set_camera(camera, cx));
                Box::new(map)
            }
            SavedItem::Terminals { rig, cols } => {
                let grid = cx.new(|cx| TerminalsItem::new(fleet, rig, window, cx));
                grid.update(cx, |grid, cx| grid.set_fixed_cols(cols, cx));
                Box::new(grid)
            }
            SavedItem::Topology {
                rig,
                pod,
                loose,
                view,
            } => {
                let scope = match (rig, pod) {
                    (Some(rig), Some(pod)) => Some(Scope::Pod { rig, pod }),
                    (Some(rig), None) => Some(Scope::Rig(rig)),
                    (None, _) => loose.then_some(Scope::Loose),
                };
                let topology = cx.new(|cx| TopologyItem::new(fleet, scope, window, cx));
                if let Some(view) = view.as_deref().and_then(SubView::from_name) {
                    topology.update(cx, |t, cx| t.show_view(view, cx));
                }
                Box::new(topology)
            }
            SavedItem::Unknown => return None,
        })
    }
}

/// What waits for ainetd's first snapshot.
struct Pending {
    center: Option<SavedMember>,
    selected: Option<String>,
}

impl Persist {
    fn write(&mut self, now: SavedWorkspace) {
        if let Err(e) = persistence::save(&self.file, &self.key, &now) {
            eprintln!(
                "ainet-gui: cannot save the workspace to {}: {e}",
                self.file.display()
            );
        }
        // Not again until it changes, even if writing failed: once said.
        self.debounce.written(now);
    }
}

/// The pane tree as saved, every tab in place: those not worth reopening
/// as `SavedItem::Unknown`, for `SavedMember::retain` to drop with the
/// active tab passed on as when restoring.
fn save_member(member: &Member<Entity<Pane>>, active: &Entity<Pane>, cx: &App) -> SavedMember {
    match member {
        Member::Pane(pane) => {
            let p = pane.read(cx);
            SavedMember::Pane {
                items: p
                    .items()
                    .map(|item| item.saved(cx).unwrap_or(SavedItem::Unknown))
                    .collect(),
                active: p.active_index(),
                active_pane: pane == active,
            }
        }
        Member::Axis(axis) => SavedMember::Axis {
            axis: match axis.axis {
                Axis::Horizontal => SavedAxis::Horizontal,
                Axis::Vertical => SavedAxis::Vertical,
            },
            members: axis.members.iter().map(|m| save_member(m, active, cx)).collect(),
            flexes: axis.flexes.borrow().clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::dock::DockPosition;
    use crate::fleet::FleetModel;
    use crate::pane_group::SplitDirection;
    use crate::persistence::{SavedWorkspace, load, save};
    use crate::tests::init_test;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ainet-gui-restore-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("gui.json")
    }

    /// A window on `fleet`, restored from `file` and saving there.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        file: &Path,
        fleet: impl FnOnce(&mut App) -> Entity<FleetModel> + 'static,
    ) -> (Entity<Workspace>, &'a mut VisualTestContext) {
        let (file, saved) = (file.to_path_buf(), load(file, "demo"));
        let (workspace, cx) = cx.add_window_view(move |window, cx| {
            let fleet = fleet(cx);
            let mut workspace = Workspace::new(fleet, None, window, cx);
            workspace.persist(file, "demo".into(), saved, true, window, cx);
            workspace
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (workspace, cx)
    }

    fn demo(cx: &mut App) -> Entity<FleetModel> {
        cx.new(FleetModel::demo)
    }

    fn saved(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> SavedWorkspace {
        workspace.read_with(cx, |w, cx| w.to_saved(cx).expect("persisting"))
    }

    fn selected(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<String> {
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        fleet.read_with(cx, |f, _| f.store.selected.clone())
    }

    fn live_agents(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<String> {
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        fleet.read_with(cx, |f, _| f.store.terminal_ids(None))
    }

    /// Every pane's tabs, by title, in order.
    fn tabs(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Vec<String>> {
        workspace.read_with(cx, |w, cx| {
            w.panes()
                .iter()
                .map(|p| {
                    p.read(cx)
                        .items()
                        .map(|i| i.tab_content_text(cx).to_string())
                        .collect()
                })
                .collect()
        })
    }

    #[gpui::test]
    async fn the_workspace_comes_back_as_it_was_left(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("round-trip");
        let left = {
            let (workspace, cx) = open(cx, &file, demo);
            let agents = live_agents(&workspace, cx);
            cx.update(|window, cx| {
                workspace.update(cx, |w, cx| {
                    w.open_inbox(None, window, cx);
                    let pane = w.active_pane().clone();
                    w.split_pane(&pane, SplitDirection::Right, window, cx);
                    w.open_terminal(&agents[0], window, cx);
                    w.open_terminal(&agents[1], window, cx);
                    let map = w.open_map(window, cx);
                    let camera = Camera {
                        zoom: 2.0,
                        pan: point(px(30.), px(-12.)),
                    };
                    map.update(cx, |m, cx| m.set_camera(camera, cx));
                    w.open_terminal(&agents[1], window, cx);
                    w.toggle_dock(DockPosition::Bottom, window, cx);
                    w.resize_dock(DockPosition::Left, px(333.), cx);
                    w.fleet_panel()
                        .update(cx, |p, cx| p.set_folded_rigs(["infra".to_string()], cx));
                    w.command_line()
                        .update(cx, |l, _| l.set_history(vec!["/ps".into(), "hello".into()]));
                });
                command_palette::set_recent(&["workspace: toggle left dock".into()], cx);
            });
            cx.run_until_parked();
            // Not the agent a new window selects first, nor one of the
            // folded rig (selecting unfolds it).
            let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
            let first = selected(&workspace, cx);
            let pick = fleet.read_with(cx, |f, _| {
                agents
                    .iter()
                    .rev()
                    .find(|id| {
                        Some(*id) != first.as_ref() && f.store.agents[*id].rig.as_deref() != Some("infra")
                    })
                    .expect("an agent to pick")
                    .clone()
            });
            fleet.update(cx, |f, cx| f.select(&pick, cx));
            cx.run_until_parked();
            let left = saved(&workspace, cx);
            assert_eq!(left.selected, Some(pick));
            workspace.update(cx, |w, cx| w.save_now(cx));
            assert_eq!(load(&file, "demo").as_ref(), Some(&left), "written as it is");
            left
        };
        let center = left.center.clone().expect("panes saved");
        let SavedMember::Axis { members, .. } = &center else {
            panic!("split in two: {center:?}")
        };
        assert_eq!(members.len(), 2);
        assert_eq!(left.folded_rigs, vec!["infra".to_string()]);
        assert_eq!(left.panel_sizes.get("FleetPanel"), Some(&333.));
        assert_eq!(left.command_history, ["/ps", "hello"]);
        assert!(left.docks.iter().any(|d| !d.open), "{:?}", left.docks);

        // A new window over the same file: the same workspace.
        cx.update(|cx| command_palette::set_recent(&[], cx));
        let (workspace, cx) = open(cx, &file, demo);
        assert_eq!(saved(&workspace, cx), left);
        let tabs = tabs(&workspace, cx);
        assert_eq!(tabs.len(), 2, "{tabs:?}");
        assert_eq!(
            tabs[0],
            vec!["Inbox".to_string()],
            "the stand-in tab is not saved"
        );
        assert!(tabs[1].iter().any(|t| t == "Map"), "{tabs:?}");
        let docks = workspace.read_with(cx, |w, cx| w.docks(cx));
        assert!(docks.fleet && docks.agent && !docks.bottom, "{docks:?}");
        let (map_zoom, active_is_terminal) = workspace.read_with(cx, |w, cx| {
            let pane = w.active_pane().read(cx);
            let map = w
                .panes()
                .iter()
                .flat_map(|p| p.read(cx).items().map(|i| i.to_any()).collect::<Vec<_>>())
                .find_map(|i| i.downcast::<MapItem>().ok())
                .expect("the map");
            let active = pane
                .active_item()
                .map(|i| i.to_any().downcast::<TerminalItem>().is_ok());
            (map.read(cx).camera().zoom, active)
        });
        assert_eq!(map_zoom, 2.0);
        assert_eq!(
            active_is_terminal,
            Some(true),
            "the active tab of the active pane"
        );
        let recent = cx.update(|_, cx| command_palette::recent(cx));
        assert_eq!(recent, vec!["workspace: toggle left dock".to_string()]);
        let history = workspace.read_with(cx, |w, cx| w.command_line().read(cx).history().to_vec());
        assert_eq!(history, ["/ps", "hello"], "the command line's lines");
        assert_eq!(selected(&workspace, cx), left.selected, "the same agent selected");
    }

    #[gpui::test]
    async fn the_topology_view_comes_back_on_its_pod_and_tab(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("topology");
        let pod = Scope::Pod {
            rig: "docs".into(),
            pod: "dev".into(),
        };
        let left = {
            let (workspace, cx) = open(cx, &file, demo);
            let scope = pod.clone();
            cx.update(|window, cx| {
                workspace.update(cx, |w, cx| {
                    let topology = w.open_topology(window, cx);
                    topology.update(cx, |t, cx| {
                        t.set_scope(scope, cx);
                        t.show_view(SubView::Recent, cx);
                    });
                })
            });
            cx.run_until_parked();
            workspace.update(cx, |w, cx| w.save_now(cx));
            saved(&workspace, cx)
        };
        let Some(SavedMember::Pane { items, .. }) = &left.center else {
            panic!("one pane: {:?}", left.center)
        };
        assert_eq!(
            items,
            &[SavedItem::Topology {
                rig: Some("docs".into()),
                pod: Some("dev".into()),
                loose: false,
                view: Some("recent".into())
            }]
        );
        let (workspace, cx) = open(cx, &file, demo);
        let topology = workspace.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<TopologyItem>().ok())
                .expect("the Topology view is back")
        });
        topology.read_with(cx, |t, _| {
            assert_eq!(t.scope(), Some(&pod));
            assert_eq!(t.view(), SubView::Recent);
            assert_eq!(t.shown().len(), 3, "the pod's seats");
        });
        assert_eq!(saved(&workspace, cx), left);
    }

    #[gpui::test]
    async fn the_terminals_grid_comes_back_with_its_rig_and_columns(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("grid");
        let left = {
            let (workspace, cx) = open(cx, &file, demo);
            let grid = cx.update(|window, cx| {
                workspace.update(cx, |w, cx| {
                    let grid = w.open_terminals(window, cx);
                    grid.update(cx, |g, cx| {
                        g.set_rig(Some("infra".into()), window, cx);
                        g.set_fixed_cols(Some(2), cx);
                    });
                    grid
                })
            });
            cx.run_until_parked();
            assert_eq!(grid.read_with(cx, |g, _| g.agents().len()), 2);
            workspace.update(cx, |w, cx| w.save_now(cx));
            saved(&workspace, cx)
        };
        let Some(SavedMember::Pane { items, .. }) = &left.center else {
            panic!("one pane: {:?}", left.center)
        };
        assert_eq!(
            items,
            &[SavedItem::Terminals {
                rig: Some("infra".into()),
                cols: Some(2)
            }],
            "the stand-in tab is not saved"
        );

        let (workspace, cx) = open(cx, &file, demo);
        let grid = workspace.read_with(cx, |w, cx| {
            w.active_pane()
                .read(cx)
                .active_item()
                .and_then(|i| i.to_any().downcast::<TerminalsItem>().ok())
                .expect("the grid is back")
        });
        let infra = live_agents(&workspace, cx);
        let fleet = workspace.read_with(cx, |w, _| w.fleet().clone());
        let infra: Vec<String> = fleet.read_with(cx, |f, _| {
            infra
                .into_iter()
                .filter(|a| f.store.agents[a].rig.as_deref() == Some("infra"))
                .collect()
        });
        grid.read_with(cx, |g, _| {
            assert_eq!(g.rig(), Some("infra"));
            assert_eq!(g.fixed_cols(), Some(2));
            assert_eq!(g.agents(), infra);
        });
        assert!(
            infra.iter().all(|a| fleet.read_with(cx, |f, _| f.is_shown(a))),
            "on show"
        );
        assert_eq!(saved(&workspace, cx), left);
    }

    #[gpui::test]
    async fn tabs_of_agents_gone_are_dropped_quietly(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("gone");
        let live = {
            let (workspace, cx) = open(cx, &file, demo);
            live_agents(&workspace, cx)[0].clone()
        };
        let terminal = |agent: &str| SavedItem::Terminal { agent: agent.into() };
        let left = SavedWorkspace {
            center: Some(SavedMember::Axis {
                axis: SavedAxis::Vertical,
                members: vec![
                    SavedMember::Pane {
                        items: vec![terminal("long-gone")],
                        active: 0,
                        active_pane: true,
                    },
                    SavedMember::Pane {
                        items: vec![terminal(&live), terminal("also-gone"), SavedItem::Comms],
                        active: 2,
                        active_pane: false,
                    },
                ],
                flexes: vec![1., 1.],
            }),
            selected: Some("long-gone".into()),
            ..SavedWorkspace::default()
        };
        save(&file, "demo", &left).unwrap();

        let (workspace, cx) = open(cx, &file, demo);
        assert_eq!(tabs(&workspace, cx).len(), 1, "the gone agent's pane went");
        assert!(selected(&workspace, cx).is_some_and(|id| id != "long-gone"));
        let restored = saved(&workspace, cx).center;
        assert_eq!(
            restored,
            Some(SavedMember::Pane {
                items: vec![terminal(&live), SavedItem::Comms],
                active: 1,
                active_pane: true,
            })
        );
        let toasts = workspace.read_with(cx, |w, cx| w.toast_layer().read(cx).texts().len());
        assert_eq!(toasts, 0, "quietly");
    }

    #[gpui::test]
    async fn changes_are_saved_once_they_settle(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("settle");
        let (workspace, cx) = open(cx, &file, demo);
        let bottom_open = |file: &Path| load(file, "demo").map(|s| s.docks.iter().all(|d| d.open));
        cx.executor().advance_clock(SAVE_CHECK * 2);
        cx.run_until_parked();
        assert_eq!(bottom_open(&file), Some(true), "a first save");

        cx.update(|window, cx| workspace.update(cx, |w, cx| w.toggle_dock(DockPosition::Bottom, window, cx)));
        cx.executor().advance_clock(SAVE_CHECK);
        cx.run_until_parked();
        assert_eq!(bottom_open(&file), Some(true), "not while it may still change");
        cx.executor().advance_clock(SAVE_CHECK);
        cx.run_until_parked();
        assert_eq!(bottom_open(&file), Some(false), "held still: written");
    }

    #[gpui::test]
    async fn closing_the_window_saves_first(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("close");
        {
            let (workspace, cx) = open(cx, &file, demo);
            workspace.update(cx, |w, cx| {
                w.fleet_panel()
                    .update(cx, |p, cx| p.set_folded_rigs(["docs".to_string()], cx))
            });
            assert!(cx.simulate_close(), "closes");
        }
        assert_eq!(load(&file, "demo").unwrap().folded_rigs, vec!["docs".to_string()]);

        let (workspace, cx) = open(cx, &file, demo);
        workspace.update(cx, |w, cx| {
            w.fleet_panel()
                .update(cx, |p, cx| p.set_folded_rigs(["infra".to_string()], cx))
        });
        cx.dispatch_action(crate::actions::CloseWindow);
        cx.run_until_parked();
        assert_eq!(
            load(&file, "demo").unwrap().folded_rigs,
            vec!["infra".to_string()]
        );
    }

    #[gpui::test]
    async fn quitting_saves_first(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("quit");
        let (workspace, cx) = open(cx, &file, demo);
        workspace.update(cx, |w, cx| {
            w.fleet_panel()
                .update(cx, |p, cx| p.set_folded_rigs(["docs".to_string()], cx))
        });
        // What quitting runs: the quit hooks, then the windows go.
        cx.cx.update(|cx| cx.shutdown());
        assert_eq!(load(&file, "demo").unwrap().folded_rigs, vec!["docs".to_string()]);
    }

    #[gpui::test]
    async fn saved_panes_wait_for_the_fleet_and_are_kept_meanwhile(cx: &mut TestAppContext) {
        init_test(cx);
        let file = temp_file("waiting");
        let left = SavedWorkspace {
            center: Some(SavedMember::Pane {
                items: vec![SavedItem::Terminal { agent: "a".into() }],
                active: 0,
                active_pane: true,
            }),
            ..SavedWorkspace::default()
        };
        save(&file, "demo", &left).unwrap();
        // No ainetd: no snapshot, so nothing to restore the terminal with.
        let (workspace, cx) = open(cx, &file, |cx| cx.new(|_| FleetModel::offline()));
        assert_eq!(tabs(&workspace, cx), vec![vec!["Fleet".to_string()]]);
        assert_eq!(saved(&workspace, cx).center, left.center, "kept for next time");
        workspace.update(cx, |w, cx| w.save_now(cx));
        assert_eq!(load(&file, "demo").unwrap().center, left.center);
    }
}
