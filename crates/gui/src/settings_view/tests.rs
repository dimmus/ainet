//! Headless tests of the Settings view: opening it, each kind of field
//! writing settings.json (comments kept) or going through ainetd, the
//! search, resets, inline errors, and the keymap editor recording keys.

use std::path::PathBuf;

use ainet_core::Request;
use gpui::{AppContext as _, Entity, Focusable, Modifiers, TestAppContext, VisualTestContext};
use serde_json::json;

use super::pages::{self, Page};
use super::{SettingsItem, Slot};
use crate::actions::{OpenKeymap, OpenSettings};
use crate::fleet::FleetModel;
use crate::item::Item;
use crate::keymap;
use crate::settings;
use crate::tests::{demo_workspace, init_test, sent};
use crate::workspace::Workspace;

/// A request as it goes on the wire, to compare.
fn wire(req: &Request) -> serde_json::Value {
    serde_json::to_value(req).unwrap()
}

/// A folder of its own for a test's files.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ainet-gui-settings-view-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// settings.json as someone wrote it: comments, a trailing comma, one of
/// the app's keys and one of ainetd's sections.
const WRITTEN: &str =
    "// Mine.\n{\n  \"ui_font_size\": 15, // big\n  // ainetd's\n  \"queue\": { \"nudge\": false },\n}\n";

/// The app saving to `dir/settings.json` (written as [`WRITTEN`]) and
/// `dir/keymap.json`.
fn files(dir: &std::path::Path, cx: &mut VisualTestContext) -> (PathBuf, PathBuf) {
    let (file, keys) = (dir.join("settings.json"), dir.join("keymap.json"));
    std::fs::write(&file, WRITTEN).unwrap();
    cx.update(|_, cx| {
        settings::save_to(file.clone(), cx);
        settings::reload(WRITTEN, cx);
        keymap::save_to(keys.clone(), cx);
    });
    (file, keys)
}

fn active<T: 'static>(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Entity<T>> {
    w.read_with(cx, |w, cx| {
        w.active_pane()
            .read(cx)
            .active_item()
            .and_then(|i| i.to_any().downcast::<T>().ok())
    })
}

fn click(selector: &str, cx: &mut VisualTestContext) {
    cx.run_until_parked();
    let at = cx
        .debug_bounds(selector.to_string().leak())
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(at.center(), Modifiers::none());
    cx.run_until_parked();
}

fn drawn(selector: &str, cx: &mut VisualTestContext) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(selector.to_string().leak()).is_some()
}

/// Types `text` into the field's input `slot` and presses enter.
fn type_in(item: &Entity<SettingsItem>, key: &str, slot: Slot, text: &str, cx: &mut VisualTestContext) {
    let input = item.read_with(cx, |s, _| s.input(key, slot));
    cx.update(|window, cx| {
        input.update(cx, |i, cx| i.set_text(text, cx));
        window.focus(&input.focus_handle(cx), cx);
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

fn text(file: &std::path::Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

#[gpui::test]
async fn opens_from_its_action_the_title_bar_and_the_palette(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    cx.dispatch_action(OpenSettings);
    let item = active::<SettingsItem>(&w, cx).expect("the Settings view");
    assert_eq!(item.read_with(cx, |s, _| s.page()), Page::Appearance);
    let search = item.read_with(cx, |s, _| s.search().clone());
    assert!(
        cx.update(|window, cx| search.focus_handle(cx).is_focused(window)),
        "the search box has the keyboard"
    );
    assert!(drawn("SETTING-theme", cx) && drawn("SETTING-ui_font_size", cx));
    // Every category is listed; ainet's say where their settings are.
    for page in Page::ALL {
        assert!(drawn(&format!("SETTINGS-NAV-{}", page.name()), cx), "{page:?}");
    }
    click("SETTINGS-NAV-retention", cx);
    assert!(drawn("SETTING-retention.inbox_items", cx));
    assert_eq!(
        item.read_with(cx, |s, cx| s.saved(cx)),
        Some(crate::persistence::SavedItem::Settings {
            page: Some("retention".into())
        })
    );

    // Opening again shows the same tab; the keymap's action opens it on
    // the Keymap.
    cx.dispatch_action(OpenKeymap);
    let again = active::<SettingsItem>(&w, cx).unwrap();
    assert_eq!(again, item);
    assert_eq!(item.read_with(cx, |s, _| s.page()), Page::Keymap);

    // The title bar's gear, from another tab.
    cx.simulate_keystrokes("ctrl-t");
    assert!(active::<SettingsItem>(&w, cx).is_none());
    click("TITLE-SETTINGS", cx);
    assert_eq!(active::<SettingsItem>(&w, cx), Some(item));

    // The palette lists it, as every action where the keyboard is.
    let names: Vec<&str> =
        cx.update(|window, cx| window.available_actions(cx).iter().map(|a| a.name()).collect());
    for name in [
        "workspace::OpenSettings",
        "workspace::OpenSettingsFile",
        "workspace::OpenKeymap",
        "workspace::OpenKeymapFile",
    ] {
        assert!(names.contains(&name), "{name}");
    }
}

#[gpui::test]
async fn toggles_dropdowns_numbers_and_text_write_the_file_keeping_comments(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let dir = temp_dir("kinds");
    let (file, _) = files(&dir, cx);
    cx.dispatch_action(OpenSettings);
    let item = active::<SettingsItem>(&w, cx).unwrap();
    let kept = |file: &std::path::Path| {
        let t = text(file);
        assert!(t.starts_with("// Mine.\n"), "{t}");
        assert!(
            t.contains("// ainetd's\n  \"queue\": { \"nudge\": false },"),
            "{t}"
        );
        t
    };

    // A toggle.
    click("SETTINGS-NAV-workspace", cx);
    assert!(!drawn("SETTING-MODIFIED-confirm_quit", cx));
    click("CONTROL-confirm_quit", cx);
    assert!(kept(&file).contains("\"confirm_quit\": true"));
    assert!(
        cx.update(|_, cx| settings::get(cx).confirm_quit),
        "in force at once"
    );
    assert!(drawn("SETTING-MODIFIED-confirm_quit", cx), "the modified dot");
    // Back to the default: the key goes from the file.
    click("CONTROL-confirm_quit", cx);
    assert!(!kept(&file).contains("confirm_quit"));
    assert!(!drawn("SETTING-MODIFIED-confirm_quit", cx));

    // A dropdown.
    click("SETTINGS-NAV-appearance", cx);
    click("CONTROL-ui_density", cx);
    click("MENU_ITEM-Compact", cx);
    assert!(kept(&file).contains("\"ui_density\": \"compact\""));
    assert_eq!(
        cx.update(|_, cx| settings::get(cx).ui_density),
        theme::UiDensity::Compact
    );

    // A number: out of range is said under it and nothing is written;
    // escape puts the value back; a good one is written.
    assert!(
        drawn("SETTING-MODIFIED-ui_font_size", cx),
        "15 is not the default"
    );
    type_in(&item, "ui_font_size", Slot::Value, "200", cx);
    assert_eq!(
        item.read_with(cx, |s, _| s.error("ui_font_size").map(String::from)),
        Some("from 6 to 100 px".into())
    );
    assert!(drawn("SETTING-ERROR-ui_font_size", cx));
    assert!(kept(&file).contains("\"ui_font_size\": 15, // big"));
    cx.simulate_keystrokes("escape");
    let input = item.read_with(cx, |s, _| s.input("ui_font_size", Slot::Value));
    assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "15");
    assert!(!drawn("SETTING-ERROR-ui_font_size", cx));
    type_in(&item, "ui_font_size", Slot::Value, "18", cx);
    assert!(
        kept(&file).contains("\"ui_font_size\": 18, // big"),
        "replaced where it stands"
    );
    assert_eq!(cx.update(|_, cx| settings::get(cx).ui_font_size), 18.);
    click("STEP-UP-ui_font_size", cx);
    assert_eq!(cx.update(|_, cx| settings::get(cx).ui_font_size), 19.);
    assert_eq!(
        input.read_with(cx, |i, _| i.text().to_string()),
        "19",
        "the input follows"
    );
    // Reset to default.
    click("SETTING-RESET-ui_font_size", cx);
    assert!(!kept(&file).contains("ui_font_size"));
    assert_eq!(cx.update(|_, cx| settings::get(cx).ui_font_size), 16.);
    assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "16");

    // Text, and a size that is unset until given.
    click("SETTINGS-NAV-terminal", cx);
    type_in(&item, "terminal.font_family", Slot::Value, "Iosevka", cx);
    assert!(kept(&file).contains("\"font_family\": \"Iosevka\""));
    assert_eq!(
        cx.update(|_, cx| settings::get(cx).terminal.font_family.to_string()),
        "Iosevka"
    );
    let size = item.read_with(cx, |s, _| s.input("terminal.font_size", Slot::Value));
    assert_eq!(
        size.read_with(cx, |i, _| i.text().to_string()),
        "",
        "follows the UI"
    );
    type_in(&item, "terminal.font_size", Slot::Value, "14", cx);
    assert_eq!(cx.update(|_, cx| settings::get(cx).terminal.font_size), Some(14.));
    type_in(&item, "terminal.font_size", Slot::Value, "", cx);
    assert_eq!(
        cx.update(|_, cx| settings::get(cx).terminal.font_size),
        None,
        "emptied: unset"
    );

    // A choice with a part of its own: a custom line height's ratio.
    assert!(!drawn("SETTING-terminal.line_height.custom", cx));
    click("CONTROL-terminal.line_height", cx);
    click("MENU_ITEM-Custom", cx);
    assert!(drawn("SETTING-terminal.line_height.custom", cx));
    type_in(&item, "terminal.line_height.custom", Slot::Value, "1.5", cx);
    assert_eq!(
        cx.update(|_, cx| settings::get(cx).terminal.line_height.ratio()),
        1.5
    );
    assert!(kept(&file).contains("{\"custom\":1.5}"), "{}", text(&file));
    click("SETTING-RESET-terminal.line_height", cx);
    assert!(!drawn("SETTING-terminal.line_height.custom", cx));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
async fn a_broken_file_is_left_alone_and_said_so(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let dir = temp_dir("broken");
    let (file, _) = files(&dir, cx);
    cx.dispatch_action(OpenSettings);
    let broken = "{ \"ui_font_size\": ";
    std::fs::write(&file, broken).unwrap();
    cx.update(|_, cx| settings::reload(broken, cx));
    let item = active::<SettingsItem>(&w, cx).unwrap();
    assert!(drawn("SETTINGS-NAV-appearance", cx));
    click("CONTROL-ui_density", cx);
    click("MENU_ITEM-Compact", cx);
    assert_eq!(text(&file), broken, "nothing written");
    let error = item
        .read_with(cx, |s, _| s.error("ui_density").map(String::from))
        .unwrap();
    assert!(error.contains("does not parse"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
async fn search_finds_fields_in_every_category(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    cx.dispatch_action(OpenSettings);
    let item = active::<SettingsItem>(&w, cx).unwrap();
    cx.simulate_input("cursor");
    assert!(drawn("SETTING-terminal.cursor_shape", cx) && drawn("SETTING-terminal.blinking", cx));
    assert!(!drawn("SETTING-ui_font_size", cx), "only the matches");
    // By description and by key, from another category.
    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("agent panel");
    assert!(drawn("SETTING-fleet_panel.dock", cx), "its description");
    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("fleet_panel.width");
    assert!(drawn("SETTING-fleet_panel.width", cx));
    // Nothing found.
    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("no such setting");
    assert!(!drawn("SETTING-theme", cx));
    // Key bindings match too, and are a click away.
    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("open map");
    click("SETTINGS-KEYMAP-MATCHES", cx);
    assert_eq!(item.read_with(cx, |s, _| s.page()), Page::Keymap);
    let keymap = item.read_with(cx, |s, _| s.keymap().clone());
    let actions: Vec<String> = keymap.read_with(cx, |k, _| {
        k.shown_rows()
            .iter()
            .map(|r| r.action.clone().unwrap_or_default())
            .collect()
    });
    assert!(actions.iter().any(|a| a == "workspace::OpenMap"), "{actions:?}");
    // Escape in the search clears it.
    click("SETTINGS-NAV-appearance", cx);
    cx.update(|window, cx| {
        let search = item.read(cx).search().clone();
        window.focus(&search.focus_handle(cx), cx);
    });
    cx.simulate_input("theme");
    cx.simulate_keystrokes("escape");
    assert!(item.read_with(cx, |s, cx| s.search().read(cx).is_empty()));
    assert!(drawn("SETTING-ui_font_size", cx), "the category again");
}

/// The Settings view, open on `page`.
fn open_on(w: &Entity<Workspace>, page: Page, cx: &mut VisualTestContext) -> Entity<SettingsItem> {
    cx.dispatch_action(OpenSettings);
    let item = active::<SettingsItem>(w, cx).expect("the Settings view");
    item.update(cx, |s, cx| s.show_page(page, cx));
    cx.run_until_parked();
    item
}

#[gpui::test]
async fn ainets_settings_go_through_ainetd(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_on(&w, Page::Queue, cx);
    assert!(
        sent(&w, cx)
            .iter()
            .any(|r| wire(r) == wire(&Request::SettingsGet { key: None })),
        "asked for them"
    );
    let last = |cx: &mut VisualTestContext| wire(sent(&w, cx).last().unwrap());

    // A toggle and a dropdown.
    click("CONTROL-queue.nudge", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "queue.nudge".into(),
            value: json!(false)
        })
    );
    assert!(
        drawn("SETTING-MODIFIED-queue.nudge", cx),
        "the demo's answer is shown"
    );
    click("SETTINGS-NAV-runtimes", cx);
    click("CONTROL-runtimes.claude.effort", cx);
    click("MENU_ITEM-Max", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "runtimes.claude.effort".into(),
            value: json!("max")
        })
    );
    // Its "unset" choice unsets it.
    click("CONTROL-runtimes.claude.effort", cx);
    click("MENU_ITEM-Agent's Default", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsUnset {
            key: "runtimes.claude.effort".into()
        })
    );
    assert!(!drawn("SETTING-MODIFIED-runtimes.claude.effort", cx));

    // ainetd refusing a value: said under the field.
    click("SETTINGS-NAV-daemon", cx);
    type_in(&item, "daemon.log.level", Slot::Value, "loud", cx);
    let error = item.read_with(cx, |s, _| s.error("daemon.log.level").map(String::from));
    assert!(
        error.is_some_and(|e| e.contains("log.level")),
        "the refusal is shown"
    );
    assert!(drawn("SETTING-ERROR-daemon.log.level", cx));
    type_in(&item, "daemon.log.level", Slot::Value, "debug", cx);
    assert_eq!(
        item.read_with(cx, |s, _| s.error("daemon.log.level").map(String::from)),
        None
    );

    // A number out of the field's range is refused before asking.
    click("SETTINGS-NAV-rigs", cx);
    let asked = sent(&w, cx).len();
    type_in(&item, "rigs.keep_snapshots", Slot::Value, "0", cx);
    assert!(drawn("SETTING-ERROR-rigs.keep_snapshots", cx));
    assert_eq!(sent(&w, cx).len(), asked);
    type_in(&item, "rigs.keep_snapshots", Slot::Value, "5", cx);
    assert_eq!(
        item.read_with(cx, |s, _| s.error("rigs.keep_snapshots").map(String::from)),
        None
    );

    // A path, picked with Browse.
    click("BROWSE-rigs.default_cwd", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/srv/work")]));
    cx.run_until_parked();
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "rigs.default_cwd".into(),
            value: json!("/srv/work")
        })
    );

    // A list: added to, by typing and with Browse, then taken from (empty
    // again is the default).
    type_in(&item, "rigs.spec_dirs", Slot::NewItem, "/srv/specs", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "rigs.spec_dirs".into(),
            value: json!(["/srv/specs"])
        })
    );
    let new_item = item.read_with(cx, |s, _| s.input("rigs.spec_dirs", Slot::NewItem));
    assert_eq!(
        new_item.read_with(cx, |i, _| i.text().to_string()),
        "",
        "cleared once added"
    );
    click("BROWSE-rigs.spec_dirs", cx);
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/srv/more")]));
    cx.run_until_parked();
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "rigs.spec_dirs".into(),
            value: json!(["/srv/specs", "/srv/more"])
        })
    );
    click("REMOVE-rigs.spec_dirs-1", cx);
    click("REMOVE-rigs.spec_dirs-0", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsUnset {
            key: "rigs.spec_dirs".into()
        })
    );

    // A map: name, then value, then Add.
    click("SETTINGS-NAV-runtimes", cx);
    let key = item.read_with(cx, |s, _| s.input("runtimes.claude.env", Slot::NewKey));
    cx.update(|_, cx| key.update(cx, |i, cx| i.set_text("RUST_LOG", cx)));
    type_in(&item, "runtimes.claude.env", Slot::NewValue, "debug", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "runtimes.claude.env".into(),
            value: json!({ "RUST_LOG": "debug" })
        })
    );
    assert!(drawn("REMOVE-runtimes.claude.env-0", cx), "the entry is listed");
}

#[gpui::test]
async fn without_ainetd_ainets_settings_go_into_the_file(cx: &mut TestAppContext) {
    init_test(cx);
    let (w, cx) = cx.add_window_view(|window, cx| {
        let fleet = cx.new(|_| FleetModel::offline());
        Workspace::new(fleet, None, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    let dir = temp_dir("offline");
    let (file, _) = files(&dir, cx);
    let item = open_on(&w, Page::Queue, cx);
    assert!(sent(&w, cx).is_empty(), "nobody to ask");
    // The file's own value, read here.
    assert!(drawn("SETTING-MODIFIED-queue.nudge", cx));
    click("CONTROL-queue.nudge", cx);
    let t = text(&file);
    assert!(
        t.starts_with("// Mine.\n") && t.contains("\"ui_font_size\": 15, // big"),
        "{t}"
    );
    assert!(!t.contains("nudge"), "back to its default: {t}");
    click("SETTINGS-NAV-rigs", cx);
    type_in(&item, "rigs.spec_dirs", Slot::NewItem, "/srv/specs", cx);
    assert!(
        text(&file).contains("\"spec_dirs\": [\"/srv/specs\"]"),
        "{}",
        text(&file)
    );
    // Refused before writing.
    type_in(&item, "rigs.keep_snapshots", Slot::Value, "0", cx);
    assert!(item.read_with(cx, |s, _| s.error("rigs.keep_snapshots").is_some()));
    assert!(!text(&file).contains("keep_snapshots"));
    // What lives in ainetd says it is not running; the library too.
    assert!(drawn("PANEL-NOT-RUNNING", cx), "the spec library");
    click("SETTINGS-NAV-permissions", cx);
    assert!(drawn("PANEL-NOT-RUNNING", cx), "the /always rules");
    // A loosening change is still asked about, then written to the file.
    click("CONTROL-permissions.persist_always", cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Set Anyway");
    cx.run_until_parked();
    assert!(
        text(&file).contains("\"persist_always\": true"),
        "{}",
        text(&file)
    );
    click("SETTINGS-NAV-hooks", cx);
    assert!(drawn("PANEL-NOT-RUNNING", cx), "hooks");
    assert!(!drawn("HOOKS-INSTALL", cx));

    // Diagnostics: the paths this app would give ainetd, and the log the
    // file names, read here.
    let log = dir.join("ainetd.log");
    std::fs::write(&log, "INFO first\nINFO last words\n").unwrap();
    type_in(
        &item,
        "hooks.claude_settings",
        Slot::Value,
        "/srv/claude.json",
        cx,
    );
    let with_log = text(&file).replacen(
        "{\n",
        &format!(
            "{{\n  \"daemon\": {{ \"log\": {{ \"file\": {:?} }} }},\n",
            log.display().to_string()
        ),
        1,
    );
    std::fs::write(&file, &with_log).unwrap();
    click("SETTINGS-NAV-diagnostics", cx);
    assert!(drawn("DIAGNOSTICS-SOCKET", cx) && drawn("DIAGNOSTICS-LOG", cx));
    assert!(!drawn("DIAGNOSTICS-AINETD", cx), "no daemon to name");
    let copied = item.read_with(cx, |s, cx| s.diagnostics_text(cx));
    assert!(copied.contains("connection         starting ainetd"), "{copied}");
    assert!(copied.contains("INFO last words"), "{copied}");
    assert!(copied.contains(&log.display().to_string()), "{copied}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
async fn the_keymap_records_removes_and_resets_bindings(cx: &mut TestAppContext) {
    use crate::actions::OpenMap;
    let (w, cx) = demo_workspace(cx);
    let dir = temp_dir("keymap");
    let (_, keys) = files(&dir, cx);
    std::fs::write(&keys, "// My keys.\n[\n]\n").unwrap();
    cx.dispatch_action(OpenKeymap);
    let item = active::<SettingsItem>(&w, cx).unwrap();
    let editor = item.read_with(cx, |s, _| s.keymap().clone());
    let bound = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            cx.key_bindings()
                .borrow()
                .bindings_for_action(&OpenMap)
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(|k| k.unparse())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(bound(cx), ["ctrl-t"]);
    let row_of = |keys: &str, cx: &mut VisualTestContext| {
        editor.read_with(cx, |e, _| {
            e.shown_rows()
                .iter()
                .position(|r| r.keys == keys && r.context == "Workspace")
                .unwrap_or_else(|| panic!("{keys} is listed"))
        })
    };
    cx.update(|window, cx| editor.update(cx, |e, cx| e.set_filter("open map", window, cx)));
    let ix = row_of("ctrl-t", cx);

    // Escape gives up without writing.
    click(&format!("KEYMAP-RECORD-{ix}"), cx);
    assert!(editor.read_with(cx, |e, _| e.is_recording()));
    cx.simulate_keystrokes("ctrl-alt-m");
    assert_eq!(editor.read_with(cx, |e, _| e.recorded()), ["ctrl-alt-m"]);
    assert!(drawn("KEYMAP-RECORDING", cx));
    cx.simulate_keystrokes("escape");
    assert!(!editor.read_with(cx, |e, _| e.is_recording()));
    assert_eq!(text(&keys), "// My keys.\n[\n]\n");

    // Keys bound elsewhere are recorded, not acted on; enter saves.
    click(&format!("KEYMAP-RECORD-{ix}"), cx);
    cx.simulate_keystrokes("ctrl-b ctrl-alt-m");
    assert_eq!(
        editor.read_with(cx, |e, _| e.recorded()),
        ["ctrl-b", "ctrl-alt-m"]
    );
    cx.simulate_keystrokes("ctrl-alt-m");
    assert_eq!(
        editor.read_with(cx, |e, _| e.recorded()),
        ["ctrl-alt-m"],
        "a third starts again"
    );
    cx.simulate_keystrokes("enter");
    let written = text(&keys);
    assert!(written.starts_with("// My keys.\n"), "{written}");
    assert!(
        written.contains("\"ctrl-alt-m\": \"workspace::OpenMap\""),
        "{written}"
    );
    assert!(
        written.contains("\"ctrl-t\": null"),
        "the default's keys unbound: {written}"
    );
    assert_eq!(bound(cx), ["ctrl-alt-m"], "in force at once");
    assert!(w.read_with(cx, |w, cx| w.docks(cx).fleet), "ctrl-b was not run");

    // Reset the default's line: ctrl-t opens the map again.
    let ix = row_of("ctrl-t", cx);
    click(&format!("KEYMAP-RESET-{ix}"), cx);
    assert!(!text(&keys).contains("ctrl-t"));
    assert_eq!(bound(cx), ["ctrl-t", "ctrl-alt-m"]);

    // Removing yours takes its line out; removing a default unbinds it.
    let ix = row_of("ctrl-alt-m", cx);
    click(&format!("KEYMAP-REMOVE-{ix}"), cx);
    assert!(!text(&keys).contains("ctrl-alt-m"));
    let ix = row_of("ctrl-t", cx);
    click(&format!("KEYMAP-REMOVE-{ix}"), cx);
    assert!(text(&keys).contains("\"ctrl-t\": null"));
    assert!(bound(cx).is_empty());
    assert_eq!(editor.read_with(cx, |e, _| e.error().map(String::from)), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test]
async fn every_ainet_category_shows_its_settings(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_on(&w, Page::Daemon, cx);
    let fields: Vec<(Page, &'static str)> = pages::fields()
        .into_iter()
        .filter(|(p, _)| !p.is_app())
        .map(|(p, f)| (p, f.key))
        .collect();
    for page in Page::ALL.into_iter().filter(|p| !p.is_app()) {
        click(&format!("SETTINGS-NAV-{}", page.name()), cx);
        assert_eq!(item.read_with(cx, |s, _| s.page()), page);
        for (_, key) in fields.iter().filter(|(p, _)| *p == page) {
            assert!(drawn(&format!("SETTING-{key}"), cx), "{page:?}: {key}");
        }
    }
    // A runtime each, under its own heading.
    click("SETTINGS-NAV-runtimes", cx);
    for group in ["Claude Code", "Codex", "Shell", "Readiness"] {
        assert!(drawn(&format!("SETTINGS-GROUP-{group}"), cx), "{group}");
    }
    // The demo's file: its values, modified, and their defaults.
    assert!(drawn("SETTING-MODIFIED-runtimes.claude.effort", cx));
    let effort = item.read_with(cx, |s, cx| {
        s.value_at(super::field::Owner::Daemon, "runtimes.claude.effort", cx)
    });
    assert_eq!(effort, json!("high"));
    // Fields that can loosen safety warn of it, always.
    assert!(drawn("SETTING-CAUTION-runtimes.claude.permission_mode", cx));
    assert!(drawn("SETTING-CAUTION-runtimes.claude.allowed_tools", cx));
    assert!(!drawn("SETTING-CAUTION-runtimes.claude.model", cx));
    // The library, and where each spec comes from.
    click("SETTINGS-NAV-rigs", cx);
    assert!(sent(&w, cx).iter().any(|r| matches!(r, Request::Specs)));
    assert!(drawn("SPEC-docs", cx), "the demo's rigs as its library");
    // Search finds ainet's settings and panels across categories.
    cx.update(|window, cx| {
        let search = item.read(cx).search().clone();
        window.focus(&search.focus_handle(cx), cx);
    });
    cx.simulate_input("revoke");
    assert!(drawn("SETTING-panel.always", cx));
    cx.simulate_keystrokes("ctrl-a backspace");
    cx.simulate_input("watchdog interval");
    assert!(drawn("SETTING-queue.watchdog.default_interval_secs", cx));
    assert!(drawn("SETTING-queue.watchdog.min_interval_secs", cx));
}

#[gpui::test]
async fn a_change_is_written_and_the_value_in_force_follows(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_on(&w, Page::Messaging, cx);
    let in_force = |key: &'static str, cx: &mut VisualTestContext| {
        item.read_with(cx, |s, cx| s.value_at(super::field::Owner::Daemon, key, cx))
    };
    assert_eq!(in_force("messaging.flush_gap_ms", cx), json!(400));
    type_in(&item, "messaging.flush_gap_ms", Slot::Value, "450", cx);
    assert_eq!(
        wire(sent(&w, cx).last().unwrap()),
        wire(&Request::SettingsSet {
            key: "messaging.flush_gap_ms".into(),
            value: json!(450)
        })
    );
    assert_eq!(
        in_force("messaging.flush_gap_ms", cx),
        json!(450),
        "ainetd's answer"
    );
    assert!(drawn("SETTING-MODIFIED-messaging.flush_gap_ms", cx));
    click("STEP-UP-messaging.flush_gap_ms", cx);
    assert_eq!(in_force("messaging.flush_gap_ms", cx), json!(500));
    click("SETTING-RESET-messaging.flush_gap_ms", cx);
    assert_eq!(in_force("messaging.flush_gap_ms", cx), json!(400));

    // Cost: a price row, then a context window, each a map of its own.
    click("SETTINGS-NAV-cost", cx);
    let model = item.read_with(cx, |s, _| s.input("cost.prices", Slot::NewKey));
    cx.update(|_, cx| model.update(cx, |i, cx| i.set_text("claude-opus-5", cx)));
    for (i, price) in ["15", "75", "", "1.5"].into_iter().enumerate() {
        let input = item.read_with(cx, |s, _| s.input("cost.prices", Slot::Price(i as u8)));
        cx.update(|_, cx| input.update(cx, |t, cx| t.set_text(price, cx)));
    }
    click("ADD-cost.prices", cx);
    assert_eq!(
        in_force("cost.prices", cx),
        json!({ "claude-opus-5": { "input": 15.0, "output": 75.0, "cache_read": 1.5 } })
    );
    assert!(drawn("PRICE-cost.prices-0", cx));
    // Output left out: said, and nothing sent.
    let asked = sent(&w, cx).len();
    cx.update(|_, cx| model.update(cx, |i, cx| i.set_text("gpt-5", cx)));
    let input = item.read_with(cx, |s, _| s.input("cost.prices", Slot::Price(0)));
    cx.update(|_, cx| input.update(cx, |t, cx| t.set_text("2", cx)));
    click("ADD-cost.prices", cx);
    assert!(item.read_with(cx, |s, _| {
        s.error("cost.prices").is_some_and(|e| e.starts_with("Output"))
    }));
    assert_eq!(sent(&w, cx).len(), asked);
    click("REMOVE-cost.prices-0", cx);
    assert_eq!(
        wire(sent(&w, cx).last().unwrap()),
        wire(&Request::SettingsUnset {
            key: "cost.prices".into()
        })
    );
    let name = item.read_with(cx, |s, _| s.input("cost.context_windows", Slot::NewKey));
    cx.update(|_, cx| name.update(cx, |i, cx| i.set_text("claude-opus-5", cx)));
    type_in(&item, "cost.context_windows", Slot::NewValue, "lots", cx);
    assert!(item.read_with(cx, |s, _| s.error("cost.context_windows").is_some()));
    type_in(&item, "cost.context_windows", Slot::NewValue, "1000000", cx);
    assert_eq!(
        in_force("cost.context_windows", cx),
        json!({ "claude-opus-5": 1_000_000 })
    );
}

#[gpui::test]
async fn changes_that_loosen_safety_are_confirmed_first(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_on(&w, Page::Runtimes, cx);
    let asked = |cx: &mut VisualTestContext| sent(&w, cx).len();
    let mode = |cx: &mut VisualTestContext| {
        item.read_with(cx, |s, cx| {
            s.value_at(super::field::Owner::Daemon, "runtimes.claude.permission_mode", cx)
        })
    };

    // A mode that bypasses every check: asked, and Cancel writes nothing.
    let before = asked(cx);
    click("CONTROL-runtimes.claude.permission_mode", cx);
    click("MENU_ITEM-Bypass Permissions (unsafe)", cx);
    let (title, detail) = cx.pending_prompt().expect("asked first");
    assert!(title.contains("without asking"), "{title}");
    assert!(detail.contains("Bypass Permissions (unsafe)"), "{detail}");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(asked(cx), before, "nothing sent");
    assert_eq!(mode(cx), serde_json::Value::Null);

    // Set Anyway sets it, and it is said to be less safe.
    click("CONTROL-runtimes.claude.permission_mode", cx);
    click("MENU_ITEM-Bypass Permissions (unsafe)", cx);
    cx.simulate_prompt_answer("Set Anyway");
    cx.run_until_parked();
    assert_eq!(mode(cx), json!("bypassPermissions"));
    assert!(drawn("SETTING-LESS-SAFE-runtimes.claude.permission_mode", cx));
    // Back to a safe mode, or the default: nothing asked.
    click("CONTROL-runtimes.claude.permission_mode", cx);
    click("MENU_ITEM-Plan Only", cx);
    assert!(!cx.has_pending_prompt());
    assert_eq!(mode(cx), json!("plan"));
    click("SETTING-RESET-runtimes.claude.permission_mode", cx);
    assert!(!cx.has_pending_prompt());
    assert_eq!(mode(cx), serde_json::Value::Null);

    // A tool allowed without asking; taking it away asks nothing.
    type_in(
        &item,
        "runtimes.claude.allowed_tools",
        Slot::NewItem,
        "Bash(cargo test:*)",
        cx,
    );
    let (_, detail) = cx.pending_prompt().expect("asked first");
    assert!(detail.contains("Bash(cargo test:*)"), "{detail}");
    cx.simulate_prompt_answer("Set Anyway");
    cx.run_until_parked();
    assert_eq!(
        item.read_with(cx, |s, cx| s.value_at(
            super::field::Owner::Daemon,
            "runtimes.claude.allowed_tools",
            cx
        )),
        json!(["Bash(cargo test:*)"])
    );
    click("REMOVE-runtimes.claude.allowed_tools-0", cx);
    assert!(!cx.has_pending_prompt());

    // An argument that turns the agent's checks off; a harmless one not.
    type_in(&item, "runtimes.claude.args", Slot::NewItem, "--verbose", cx);
    assert!(!cx.has_pending_prompt());
    type_in(
        &item,
        "runtimes.claude.args",
        Slot::NewItem,
        "--dangerously-skip-permissions",
        cx,
    );
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();

    // A tool disallowed asks nothing; allowing it again does.
    type_in(
        &item,
        "runtimes.claude.disallowed_tools",
        Slot::NewItem,
        "WebFetch",
        cx,
    );
    assert!(!cx.has_pending_prompt(), "holding agents back asks nothing");
    let denied = |cx: &mut VisualTestContext| {
        item.read_with(cx, |s, cx| {
            s.value_at(
                super::field::Owner::Daemon,
                "runtimes.claude.disallowed_tools",
                cx,
            )
        })
    };
    assert_eq!(denied(cx), json!(["WebFetch"]));
    // Its Remove (below the window's fold here) is this call.
    cx.update(|window, cx| {
        item.update(cx, |s, cx| {
            s.remove_at("runtimes.claude.disallowed_tools", 0, window, cx)
        })
    });
    cx.run_until_parked();
    let (_, detail) = cx.pending_prompt().expect("asked first");
    assert!(detail.contains("WebFetch taken off"), "{detail}");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(
        item.read_with(cx, |s, cx| s.value_at(
            super::field::Owner::Daemon,
            "runtimes.claude.disallowed_tools",
            cx
        )),
        json!(["WebFetch"])
    );

    // Rules kept, and global rules: asked too.
    click("SETTINGS-NAV-permissions", cx);
    click("CONTROL-permissions.persist_always", cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Set Anyway");
    cx.run_until_parked();
    click("CONTROL-permissions.persist_always", cx);
    assert!(!cx.has_pending_prompt(), "turning it off is safer");
    type_in(
        &item,
        "permissions.global_rules",
        Slot::NewItem,
        "{\"tool\": \"WebSearch\"}",
        cx,
    );
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Set Anyway");
    cx.run_until_parked();
    assert_eq!(
        wire(sent(&w, cx).last().unwrap()),
        wire(&Request::SettingsSet {
            key: "permissions.global_rules".into(),
            value: json!([{ "tool": "WebSearch" }])
        })
    );
    // Not JSON: said, nothing asked.
    type_in(&item, "permissions.global_rules", Slot::NewItem, "{tool", cx);
    assert!(!cx.has_pending_prompt());
    assert!(item.read_with(cx, |s, _| s.error("permissions.global_rules").is_some()));
}

#[gpui::test]
async fn always_rules_are_listed_and_revoked(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let _item = open_on(&w, Page::Permissions, cx);
    assert!(
        sent(&w, cx)
            .iter()
            .any(|r| wire(r) == wire(&Request::AlwaysList { agent: None }))
    );
    assert!(drawn("ALWAYS-ROW-0", cx) && drawn("ALWAYS-ROW-1", cx));
    click("ALWAYS-REVOKE-0", cx);
    let revoke = sent(&w, cx)
        .into_iter()
        .rev()
        .find(|r| matches!(r, Request::AlwaysRevoke { .. }))
        .expect("revoked");
    assert!(
        matches!(&revoke, Request::AlwaysRevoke { rule: Some(key), .. } if key == "Bash:cargo test"),
        "{revoke:?}"
    );
    assert!(
        matches!(sent(&w, cx).last(), Some(Request::AlwaysList { agent: None })),
        "listed again"
    );
    assert!(
        drawn("ALWAYS-ROW-0", cx) && !drawn("ALWAYS-ROW-1", cx),
        "one left"
    );
    click("ALWAYS-REVOKE-0", cx);
    assert!(drawn("ALWAYS-NONE", cx));
}

#[gpui::test]
async fn hooks_change_only_once_confirmed(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let _item = open_on(&w, Page::Hooks, cx);
    assert!(matches!(sent(&w, cx).last(), Some(Request::HookStatus)));
    assert!(drawn("HOOKS-SUMMARY", cx));
    assert!(drawn("HOOKS-EVENT-PermissionRequest", cx) && drawn("HOOKS-EVENT-SessionStart", cx));
    assert!(!drawn("HOOKS-UNINSTALL", cx), "nothing to take out");

    // The file and its backup are named before anything is written.
    click("HOOKS-INSTALL", cx);
    let (title, detail) = cx.pending_prompt().expect("asked first");
    assert_eq!(title, "Install ainet's hooks?");
    assert!(detail.contains(".claude/settings.json"), "{detail}");
    assert!(detail.contains("settings.json.bak.ainet."), "{detail}");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!sent(&w, cx).iter().any(|r| matches!(r, Request::HookInstall)));

    click("HOOKS-INSTALL", cx);
    cx.simulate_prompt_answer("Install");
    cx.run_until_parked();
    assert!(matches!(sent(&w, cx).last(), Some(Request::HookInstall)));
    assert!(drawn("HOOKS-DONE", cx), "what was done, and the backup");
    assert!(drawn("HOOKS-UNINSTALL", cx) && !drawn("HOOKS-INSTALL", cx));

    click("HOOKS-UNINSTALL", cx);
    let (title, _) = cx.pending_prompt().expect("asked first");
    assert_eq!(title, "Uninstall ainet's hooks?");
    cx.simulate_prompt_answer("Uninstall");
    cx.run_until_parked();
    assert!(matches!(sent(&w, cx).last(), Some(Request::HookUninstall)));
    assert!(drawn("HOOKS-INSTALL", cx));
}

#[gpui::test]
async fn diagnostics_show_the_daemon_and_copy_without_secrets(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_on(&w, Page::Diagnostics, cx);
    let asked = sent(&w, cx);
    assert!(asked.iter().any(|r| matches!(r, Request::Paths)));
    assert!(asked.iter().any(|r| matches!(r, Request::LogTail { .. })));
    for row in ["CONNECTION", "DESKTOP-APP", "AINETD", "SOCKET", "SETTINGS", "LOG"] {
        assert!(drawn(&format!("DIAGNOSTICS-{row}"), cx), "{row}");
    }
    // A secret in the environment of seats is not copied.
    item.update(cx, |s, cx| {
        s.write("runtimes.claude.env", Some(json!({ "API_KEY": "sekrit" })), cx)
    });
    cx.run_until_parked();
    click("DIAGNOSTICS-COPY", cx);
    assert!(drawn("DIAGNOSTICS-COPIED", cx));
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).expect("copied");
    for want in [
        "ainet diagnostics",
        "connection         the demo fleet",
        "pid 4040001, up 3 h 12 min",
        "runtimes.claude.effort = \"high\"",
        "runtimes.claude.env = (set)",
        "listening on /run/user/1000/ainet.sock",
    ] {
        assert!(copied.contains(want), "{want} in:\n{copied}");
    }
    assert!(!copied.contains("sekrit"), "{copied}");
    let before = sent(&w, cx).len();
    click("DIAGNOSTICS-REFRESH", cx);
    assert_eq!(sent(&w, cx).len(), before + 2, "paths and log asked again");
}

#[gpui::test]
async fn a_daemon_that_cannot_answer_is_said_so_where_it_is_needed(cx: &mut TestAppContext) {
    init_test(cx);
    let (w, cx) = cx.add_window_view(|window, cx| {
        let fleet = cx.new(|cx| FleetModel::demo_with(ainet_model::Demo::new(), cx));
        Workspace::new(fleet, None, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    let _item = open_on(&w, Page::Permissions, cx);
    assert!(drawn("ALWAYS-ERROR", cx));
    click("SETTINGS-NAV-hooks", cx);
    assert!(drawn("HOOKS-ERROR", cx) && !drawn("HOOKS-INSTALL", cx));
    click("SETTINGS-NAV-diagnostics", cx);
    assert!(
        drawn("DIAGNOSTICS-PATHS", cx),
        "the reason, where the paths would be"
    );
    // The settings themselves still show, from the file.
    click("SETTINGS-NAV-queue", cx);
    assert!(drawn("SETTING-queue.nudge", cx));
}
