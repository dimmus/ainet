//! Headless tests of the Settings view: opening it, each kind of field
//! writing settings.json (comments kept) or going through ainetd, the
//! search, resets, inline errors, and the keymap editor recording keys.

use std::path::PathBuf;

use ainet_core::Request;
use gpui::{AppContext as _, Entity, Focusable, Modifiers, TestAppContext, VisualTestContext};
use serde_json::json;

use super::field::{Choice, Field, Kind};
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
    assert!(drawn("SETTINGS-PLACEHOLDER-retention", cx));
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

/// ainetd's settings of every other kind, on a category of their own.
fn daemon_fields() -> Vec<(Page, Field)> {
    let mut fields = pages::fields();
    fields.extend(
        [
            Field::daemon("queue.nudge", "Nudge", Kind::Toggle),
            Field::daemon(
                "runtimes.claude.permission_mode",
                "Permission Mode",
                Kind::Dropdown(vec![
                    Choice::text("Default", "default"),
                    Choice::text("Bypass Permissions", "bypassPermissions"),
                ]),
            ),
            Field::daemon(
                "rigs.keep_snapshots",
                "Snapshots Kept",
                Kind::Number {
                    unit: "",
                    min: 0.,
                    max: 1000.,
                    step: 1.,
                    integer: true,
                },
            ),
            Field::daemon(
                "rigs.default_cwd",
                "Default Folder",
                Kind::Path { directory: true },
            ),
            Field::daemon(
                "rigs.spec_dirs",
                "Spec Folders",
                Kind::List {
                    placeholder: "a folder",
                },
            ),
            Field::daemon(
                "runtimes.claude.env",
                "Environment",
                Kind::Map {
                    key_placeholder: "NAME",
                    value_placeholder: "value",
                },
            )
            .describe("Variables for every Claude Code seat."),
        ]
        .into_iter()
        .map(|f| (Page::Rigs, f)),
    );
    fields
}

/// The Settings view on [`daemon_fields`], on the Rigs category.
fn open_daemon_fields(w: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<SettingsItem> {
    let item = w.update_in(cx, |w, window, cx| {
        let fleet = w.fleet().clone();
        let item =
            cx.new(|cx| SettingsItem::with_fields(fleet, Some(Page::Rigs), daemon_fields(), window, cx));
        w.active_pane().update(cx, |p, cx| {
            p.add_item(Box::new(item.clone()), true, true, window, cx)
        });
        item
    });
    cx.run_until_parked();
    item
}

#[gpui::test]
async fn ainets_settings_go_through_ainetd(cx: &mut TestAppContext) {
    let (w, cx) = demo_workspace(cx);
    let item = open_daemon_fields(&w, cx);
    assert!(
        sent(&w, cx)
            .iter()
            .any(|r| wire(r) == wire(&Request::SettingsGet { key: None })),
        "asked for them"
    );
    let last = |cx: &mut VisualTestContext| wire(sent(&w, cx).last().unwrap());

    // A toggle and a dropdown; a value that lets agents act without
    // asking is said to.
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
    click("CONTROL-runtimes.claude.permission_mode", cx);
    click("MENU_ITEM-Bypass Permissions", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsSet {
            key: "runtimes.claude.permission_mode".into(),
            value: json!("bypassPermissions")
        })
    );
    assert!(drawn("SETTING-LESS-SAFE-runtimes.claude.permission_mode", cx));
    click("SETTING-RESET-runtimes.claude.permission_mode", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsUnset {
            key: "runtimes.claude.permission_mode".into()
        })
    );
    assert!(!drawn("SETTING-LESS-SAFE-runtimes.claude.permission_mode", cx));

    // ainetd refusing a value: said under the field.
    type_in(&item, "rigs.keep_snapshots", Slot::Value, "0", cx);
    let error = item.read_with(cx, |s, _| s.error("rigs.keep_snapshots").map(String::from));
    assert!(error.is_some(), "the refusal is shown");
    assert!(drawn("SETTING-ERROR-rigs.keep_snapshots", cx));
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

    // A list: added to, then taken from (empty again is the default).
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
    click("REMOVE-rigs.spec_dirs-0", cx);
    assert_eq!(
        last(cx),
        wire(&Request::SettingsUnset {
            key: "rigs.spec_dirs".into()
        })
    );

    // A map: name, then value, then Add.
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
    let item = open_daemon_fields(&w, cx);
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
