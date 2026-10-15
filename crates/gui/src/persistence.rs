//! The workspace as it was left, restored at the next start: after Zed's
//! `workspace::persistence`, but one JSON file instead of SQLite, and only
//! what ainet's window has. `$XDG_STATE_HOME/ainet/gui.json` keeps one
//! workspace per daemon socket (and one for the demo): the window's bounds,
//! the docks, the panes with their splits and tabs, the Fleet's folded rigs
//! and selected agent, the Map's view and the palette's recent actions.
//!
//! It is state, not configuration: a file that does not parse, or was
//! written in another format version, is ignored and written anew; a file
//! from a newer version of the app is left alone. Tabs whose agent is gone
//! are dropped when restored, quietly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

/// The file's format. Another number is not read (and is written over,
/// unless it is newer).
pub const VERSION: u32 = 1;

/// The palette keeps this many recent actions.
pub const RECENT_ACTIONS: usize = 50;

/// `$XDG_STATE_HOME/ainet/gui.json`.
pub fn state_file() -> PathBuf {
    ainet_core::paths::state_dir().join("gui.json")
}

/// Which workspace of the file a window is: the daemon's socket, or the
/// demo.
pub fn key(demo: bool, socket: &Path) -> String {
    if demo {
        "demo".to_string()
    } else {
        socket.display().to_string()
    }
}

/// One window's workspace. Every field is optional, so a file missing one
/// still restores the rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWorkspace {
    pub window: Option<SavedWindow>,
    /// Each dock: the panel showing, and whether it is open. By panel, not
    /// by side, so moving the Fleet to the other side keeps it.
    pub docks: Vec<SavedDock>,
    /// The panels dragged to a size of their own, by name.
    pub panel_sizes: BTreeMap<String, f32>,
    /// The panes; `None` keeps the one a new window opens with.
    pub center: Option<SavedMember>,
    /// The Fleet's folded rigs, sorted.
    pub folded_rigs: Vec<String>,
    /// The agent selected in the Fleet, by id.
    pub selected: Option<String>,
    /// What was run from the palette, the latest last.
    pub recent_actions: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub state: WindowState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedDock {
    pub panel: String,
    pub open: bool,
}

/// The pane tree, as `pane_group::Member`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedMember {
    Pane {
        items: Vec<SavedItem>,
        /// The active tab.
        #[serde(default)]
        active: usize,
        /// Whether this is the workspace's active pane.
        #[serde(default)]
        active_pane: bool,
    },
    Axis {
        axis: SavedAxis,
        members: Vec<SavedMember>,
        #[serde(default)]
        flexes: Vec<f32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedAxis {
    Horizontal,
    Vertical,
}

/// A tab worth reopening. Reports and stand-ins are not saved: a report is
/// one reply, asked for again in a moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedItem {
    Terminal {
        agent: String,
    },
    Inbox,
    Comms,
    Map {
        zoom: f32,
        pan_x: f32,
        pan_y: f32,
    },
    /// A kind this version does not know (a newer app's): dropped.
    #[serde(other)]
    Unknown,
}

impl SavedWindow {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let (b, state) = match bounds {
            WindowBounds::Windowed(b) => (b, WindowState::Windowed),
            WindowBounds::Maximized(b) => (b, WindowState::Maximized),
            WindowBounds::Fullscreen(b) => (b, WindowState::Fullscreen),
        };
        SavedWindow {
            x: b.origin.x.as_f32(),
            y: b.origin.y.as_f32(),
            width: b.size.width.as_f32(),
            height: b.size.height.as_f32(),
            state,
        }
    }

    /// The bounds to open the window with, at least `min` big. A window
    /// last seen on a display that is gone (`displays` are the ones there
    /// now) keeps its size but is centred on `fallback`, the main display.
    pub fn to_bounds(
        self,
        min: gpui::Size<Pixels>,
        displays: &[Bounds<Pixels>],
        fallback: Option<Bounds<Pixels>>,
    ) -> Option<WindowBounds> {
        let finite = [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite());
        if !finite {
            return None;
        }
        let size = size(px(self.width).max(min.width), px(self.height).max(min.height));
        let origin = point(px(self.x), px(self.y));
        let on_a_display = displays.is_empty() || displays.iter().any(|d| d.contains(&origin));
        let bounds = match fallback.filter(|_| !on_a_display) {
            Some(display) => Bounds::centered_at(display.center(), size),
            None => Bounds::new(origin, size),
        };
        Some(match self.state {
            WindowState::Windowed => WindowBounds::Windowed(bounds),
            WindowState::Maximized => WindowBounds::Maximized(bounds),
            WindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
        })
    }
}

impl SavedMember {
    /// The tree with only the tabs `keep` says yes to: panes left empty
    /// go, an axis left with one member becomes that member, and the
    /// survivors keep their shares. The active tab stays active if kept,
    /// else the one before it; the active pane, if it goes, passes to the
    /// first pane left.
    pub fn retain(self, keep: &dyn Fn(&SavedItem) -> bool) -> Option<SavedMember> {
        let mut member = self.retain_inner(keep)?;
        if !member.has_active_pane() {
            member.mark_first_active();
        }
        Some(member)
    }

    fn retain_inner(self, keep: &dyn Fn(&SavedItem) -> bool) -> Option<SavedMember> {
        match self {
            SavedMember::Pane {
                items,
                active,
                active_pane,
            } => {
                let before_active = items[..active.min(items.len())]
                    .iter()
                    .filter(|i| keep(i))
                    .count();
                let active_kept = items.get(active).is_some_and(keep);
                let items: Vec<SavedItem> = items.into_iter().filter(|i| keep(i)).collect();
                if items.is_empty() {
                    return None;
                }
                let active = if active_kept {
                    before_active
                } else {
                    before_active.saturating_sub(1)
                };
                Some(SavedMember::Pane {
                    active: active.min(items.len() - 1),
                    items,
                    active_pane,
                })
            }
            SavedMember::Axis {
                axis,
                members,
                flexes,
            } => {
                let flexes = flexes_for(&flexes, members.len());
                let (mut members, kept_flexes): (Vec<_>, Vec<_>) = members
                    .into_iter()
                    .zip(flexes)
                    .filter_map(|(m, f)| Some((m.retain_inner(keep)?, f)))
                    .unzip();
                match members.len() {
                    0 => None,
                    1 => members.pop(),
                    n => Some(SavedMember::Axis {
                        axis,
                        flexes: flexes_for(&kept_flexes, n),
                        members,
                    }),
                }
            }
        }
    }

    fn has_active_pane(&self) -> bool {
        match self {
            SavedMember::Pane { active_pane, .. } => *active_pane,
            SavedMember::Axis { members, .. } => members.iter().any(Self::has_active_pane),
        }
    }

    fn mark_first_active(&mut self) {
        match self {
            SavedMember::Pane { active_pane, .. } => *active_pane = true,
            SavedMember::Axis { members, .. } => {
                if let Some(first) = members.first_mut() {
                    first.mark_first_active();
                }
            }
        }
    }
}

/// `flexes` as an axis of `n` members takes them: the same shares, adding
/// up to `n` as `pane_group` keeps them; even shares if they do not fit.
pub fn flexes_for(flexes: &[f32], n: usize) -> Vec<f32> {
    let sum: f32 = flexes.iter().sum();
    let fit = flexes.len() == n && flexes.iter().all(|f| f.is_finite() && *f > 0.) && sum > 0.;
    if fit {
        flexes.iter().map(|f| f * n as f32 / sum).collect()
    } else {
        vec![1.; n]
    }
}

/// Decides when to write: once the workspace has changed from what was
/// written and then held still for one check, so dragging a divider
/// writes once, at the end.
#[derive(Default)]
pub struct Debounce {
    written: Option<SavedWorkspace>,
    seen: Option<SavedWorkspace>,
}

impl Debounce {
    /// Starts from what the file holds.
    pub fn new(written: Option<SavedWorkspace>) -> Self {
        Debounce {
            seen: written.clone(),
            written,
        }
    }

    /// `now` if it should be written now.
    pub fn check(&mut self, now: SavedWorkspace) -> Option<SavedWorkspace> {
        let settled = self.seen.as_ref() == Some(&now);
        self.seen = Some(now.clone());
        (settled && self.written.as_ref() != Some(&now)).then_some(now)
    }

    /// `now` was written.
    pub fn written(&mut self, now: SavedWorkspace) {
        self.seen = Some(now.clone());
        self.written = Some(now);
    }

    /// Whether `now` is what was written last.
    pub fn is_written(&self, now: &SavedWorkspace) -> bool {
        self.written.as_ref() == Some(now)
    }
}

/// The file: a version and the workspaces by key, each kept as JSON so
/// one this version cannot read does not lose the others.
#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    version: u32,
    #[serde(default)]
    workspaces: BTreeMap<String, serde_json::Value>,
}

/// What `file` says, if it is this version's; `Err` for a newer one.
fn read(file: &Path) -> Result<Option<StateFile>, ()> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Ok(None);
    };
    let Ok(state) = serde_json::from_str::<StateFile>(&text) else {
        return Ok(None);
    };
    match state.version {
        VERSION => Ok(Some(state)),
        v if v > VERSION => Err(()),
        _ => Ok(None),
    }
}

/// The workspace saved under `key`, if the file has one this version can
/// read.
pub fn load(file: &Path, key: &str) -> Option<SavedWorkspace> {
    let mut state = read(file).ok().flatten()?;
    serde_json::from_value(state.workspaces.remove(key)?).ok()
}

/// Saves `workspace` under `key`, keeping the file's other workspaces.
/// Written whole or not at all. A file from a newer version of the app is
/// left alone.
pub fn save(file: &Path, key: &str, workspace: &SavedWorkspace) -> std::io::Result<()> {
    let mut state = match read(file) {
        Ok(state) => state.unwrap_or_default(),
        Err(()) => return Ok(()),
    };
    state.version = VERSION;
    state
        .workspaces
        .insert(key.to_string(), serde_json::to_value(workspace)?);
    let text = serde_json::to_string_pretty(&state)?;
    crate::settings::write_whole(file, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(agent: &str) -> SavedItem {
        SavedItem::Terminal { agent: agent.into() }
    }

    fn pane(items: Vec<SavedItem>, active: usize, active_pane: bool) -> SavedMember {
        SavedMember::Pane {
            items,
            active,
            active_pane,
        }
    }

    fn sample() -> SavedWorkspace {
        SavedWorkspace {
            window: Some(SavedWindow {
                x: 10.,
                y: 20.,
                width: 1400.,
                height: 900.,
                state: WindowState::Maximized,
            }),
            docks: vec![
                SavedDock {
                    panel: "FleetPanel".into(),
                    open: false,
                },
                SavedDock {
                    panel: "EventsPanel".into(),
                    open: true,
                },
            ],
            panel_sizes: BTreeMap::from([("FleetPanel".into(), 312.)]),
            center: Some(SavedMember::Axis {
                axis: SavedAxis::Horizontal,
                members: vec![
                    pane(vec![terminal("a"), SavedItem::Inbox], 1, false),
                    SavedMember::Axis {
                        axis: SavedAxis::Vertical,
                        members: vec![
                            pane(vec![SavedItem::Comms], 0, true),
                            pane(
                                vec![SavedItem::Map {
                                    zoom: 2.,
                                    pan_x: 5.,
                                    pan_y: -7.,
                                }],
                                0,
                                false,
                            ),
                        ],
                        flexes: vec![0.5, 1.5],
                    },
                ],
                flexes: vec![1.2, 0.8],
            }),
            folded_rigs: vec!["infra".into()],
            selected: Some("a".into()),
            recent_actions: vec!["workspace: toggle left dock".into()],
        }
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ainet-gui-persistence-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("ainet").join("gui.json")
    }

    #[test]
    fn a_workspace_round_trips_through_the_file() {
        let file = temp_file("round-trip");
        assert_eq!(load(&file, "demo"), None, "no file yet");
        save(&file, "demo", &sample()).unwrap();
        assert_eq!(load(&file, "demo"), Some(sample()));
        assert_eq!(load(&file, "/run/user/1000/ainet.sock"), None);

        // Another socket's workspace sits beside it.
        let other = SavedWorkspace {
            folded_rigs: vec!["docs".into()],
            ..SavedWorkspace::default()
        };
        save(&file, "/tmp/other.sock", &other).unwrap();
        assert_eq!(load(&file, "demo"), Some(sample()));
        assert_eq!(load(&file, "/tmp/other.sock"), Some(other));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"version\": 1"), "{text}");
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["gui.json"], "nothing left beside it");
    }

    #[test]
    fn corrupt_and_foreign_files_are_ignored() {
        let file = temp_file("corrupt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        for text in ["", "{", "[1, 2]", r#"{ "workspaces": {} }"#, "\u{0}garbage"] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(load(&file, "demo"), None, "{text:?}");
        }
        // Written anew over a corrupt file.
        save(&file, "demo", &sample()).unwrap();
        assert_eq!(load(&file, "demo"), Some(sample()));

        // An older format is ignored, and replaced.
        std::fs::write(&file, r#"{ "version": 0, "workspaces": { "demo": {} } }"#).unwrap();
        assert_eq!(load(&file, "demo"), None);
        save(&file, "demo", &sample()).unwrap();
        assert_eq!(load(&file, "demo"), Some(sample()));

        // A newer app's file is neither read nor written over.
        let newer = r#"{ "version": 99, "workspaces": { "demo": { "folded_rigs": ["x"] } } }"#;
        std::fs::write(&file, newer).unwrap();
        assert_eq!(load(&file, "demo"), None);
        save(&file, "demo", &sample()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), newer);

        // A workspace that does not parse costs only itself.
        let mixed = r#"{ "version": 1, "workspaces": {
            "bad": { "docks": 7 },
            "good": { "folded_rigs": ["docs"], "something_new": true } } }"#;
        std::fs::write(&file, mixed).unwrap();
        assert_eq!(load(&file, "bad"), None);
        assert_eq!(load(&file, "good").unwrap().folded_rigs, vec!["docs".to_string()]);
    }

    #[test]
    fn unknown_tab_kinds_are_dropped() {
        let json = r#"{ "kind": "pane", "items": [
            { "kind": "terminal", "agent": "a" }, { "kind": "hologram", "x": 1 } ], "active": 1 }"#;
        let member: SavedMember = serde_json::from_str(json).unwrap();
        let kept = member.retain(&|i| *i != SavedItem::Unknown).unwrap();
        assert_eq!(kept, pane(vec![terminal("a")], 0, true));
    }

    #[test]
    fn tabs_of_agents_gone_are_dropped_and_the_tree_folds() {
        let center = sample().center.unwrap();
        let gone = |agent: &'static str| move |i: &SavedItem| *i != terminal(agent);

        // Nothing gone: the same tree.
        assert_eq!(center.clone().retain(&|_| true), Some(center.clone()));

        // `a` gone: the Inbox is left in its pane, active.
        let SavedMember::Axis { members, .. } = center.clone().retain(&gone("a")).unwrap() else {
            panic!("still split");
        };
        assert_eq!(members[0], pane(vec![SavedItem::Inbox], 0, false));

        // Only terminals of gone agents: the panes holding nothing else go
        // and the axes fold, keeping the survivors' shares.
        let only_terminals = |i: &SavedItem| matches!(i, SavedItem::Terminal { .. });
        let kept = center
            .clone()
            .retain(&|i| !only_terminals(i) && *i != SavedItem::Inbox);
        assert_eq!(
            kept,
            Some(SavedMember::Axis {
                axis: SavedAxis::Vertical,
                members: vec![
                    pane(vec![SavedItem::Comms], 0, true),
                    pane(
                        vec![SavedItem::Map {
                            zoom: 2.,
                            pan_x: 5.,
                            pan_y: -7.
                        }],
                        0,
                        false
                    ),
                ],
                flexes: vec![0.5, 1.5],
            })
        );

        // The active pane gone: the first pane left takes over.
        let kept = center.clone().retain(&|i| *i != SavedItem::Comms).unwrap();
        let SavedMember::Axis { members, .. } = kept else {
            panic!("still split");
        };
        assert!(matches!(
            members[0],
            SavedMember::Pane {
                active_pane: true,
                ..
            }
        ));

        // Everything gone: nothing to restore.
        assert_eq!(center.retain(&|_| false), None);
    }

    #[test]
    fn the_active_tab_stays_or_passes_to_the_one_before() {
        let items = vec![terminal("a"), terminal("b"), terminal("c"), terminal("d")];
        let keep_active = pane(items.clone(), 2, true).retain(&|i| *i != terminal("a"));
        assert_eq!(
            keep_active,
            Some(pane(vec![terminal("b"), terminal("c"), terminal("d")], 1, true))
        );
        let active_gone = pane(items.clone(), 2, true).retain(&|i| *i != terminal("c"));
        assert_eq!(
            active_gone,
            Some(pane(vec![terminal("a"), terminal("b"), terminal("d")], 1, true))
        );
        let first_gone = pane(items.clone(), 0, true).retain(&|i| *i != terminal("a"));
        assert_eq!(
            first_gone,
            Some(pane(vec![terminal("b"), terminal("c"), terminal("d")], 0, true))
        );
        let out_of_range = pane(items, 9, true).retain(&|_| true).unwrap();
        assert!(matches!(out_of_range, SavedMember::Pane { active: 3, .. }));
    }

    #[test]
    fn flexes_keep_their_shares_or_even_out() {
        assert_eq!(flexes_for(&[1., 3.], 2), vec![0.5, 1.5]);
        assert_eq!(flexes_for(&[0.5, 1.5], 2), vec![0.5, 1.5]);
        assert_eq!(flexes_for(&[1.], 2), vec![1., 1.]);
        assert_eq!(flexes_for(&[1., f32::NAN], 2), vec![1., 1.]);
        assert_eq!(flexes_for(&[1., -1.], 2), vec![1., 1.]);
        assert_eq!(flexes_for(&[], 3), vec![1., 1., 1.]);
    }

    #[test]
    fn window_bounds_come_back_within_reason() {
        let min = size(px(480.), px(320.));
        let display = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let saved = SavedWindow::from_bounds(WindowBounds::Maximized(Bounds::new(
            point(px(100.), px(50.)),
            size(px(1200.), px(700.)),
        )));
        assert_eq!(saved.state, WindowState::Maximized);
        assert_eq!(
            saved.to_bounds(min, &[display], Some(display)),
            Some(WindowBounds::Maximized(Bounds::new(
                point(px(100.), px(50.)),
                size(px(1200.), px(700.))
            )))
        );
        // Too small: the window's least.
        let tiny = SavedWindow {
            width: 10.,
            height: 10.,
            ..saved
        };
        let Some(WindowBounds::Maximized(b)) = tiny.to_bounds(min, &[display], None) else {
            panic!()
        };
        assert_eq!(b.size, min);
        // On a display that is gone: centred on the main one, same size.
        let away = SavedWindow {
            x: 5000.,
            state: WindowState::Windowed,
            ..saved
        };
        let Some(WindowBounds::Windowed(b)) = away.to_bounds(min, &[display], Some(display)) else {
            panic!()
        };
        assert_eq!(b.center(), display.center());
        assert_eq!(b.size, size(px(1200.), px(700.)));
        // Not a number: the default instead.
        let nan = SavedWindow { x: f32::NAN, ..saved };
        assert_eq!(nan.to_bounds(min, &[display], None), None);
    }

    #[test]
    fn writes_wait_for_the_workspace_to_hold_still() {
        let start = SavedWorkspace::default();
        let mut debounce = Debounce::new(Some(start.clone()));
        assert_eq!(debounce.check(start.clone()), None, "nothing changed");

        let moved = |n: f32| SavedWorkspace {
            panel_sizes: BTreeMap::from([("FleetPanel".into(), n)]),
            ..SavedWorkspace::default()
        };
        assert_eq!(debounce.check(moved(300.)), None, "still moving");
        assert_eq!(debounce.check(moved(320.)), None, "still moving");
        assert_eq!(debounce.check(moved(320.)), Some(moved(320.)), "settled");
        debounce.written(moved(320.));
        assert!(debounce.is_written(&moved(320.)));
        assert_eq!(debounce.check(moved(320.)), None, "written already");

        // Back to what was written before it settled elsewhere: nothing.
        assert_eq!(debounce.check(moved(330.)), None);
        assert_eq!(debounce.check(moved(320.)), None);
        assert_eq!(debounce.check(moved(320.)), None);

        // No file yet: the first settled state is written.
        let mut fresh = Debounce::new(None);
        assert_eq!(fresh.check(start.clone()), None);
        assert_eq!(fresh.check(start.clone()), Some(start));
    }

    #[test]
    fn keys_are_the_socket_or_the_demo() {
        assert_eq!(key(true, Path::new("/x.sock")), "demo");
        assert_eq!(
            key(false, Path::new("/run/user/1000/ainet.sock")),
            "/run/user/1000/ainet.sock"
        );
    }
}
