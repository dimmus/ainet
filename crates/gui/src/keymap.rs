//! Key bindings in Zed's keymap format: a JSON array of sections, each with
//! an optional `context` (`Workspace`, `FleetPanel`, `Terminal`, `Inbox`,
//! `Comms`, `Picker`, `TextInput`) and `bindings` from keystrokes to an action name,
//! `[name, input]`, or `null` to unbind. The platform's default keymap is
//! bundled; `~/.config/ainet/keymap.json` adds to it and overrides it, and
//! is followed for changes. Because the bindings live in GPUI's keymap,
//! `ui::KeyBinding::for_action` shows them as hints anywhere.

use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    Action, App, Global, KeyBinding, KeyBindingContextPredicate, KeyBindingMetaIndex, NoAction, SharedString,
    Task,
};
use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::Value;

use crate::assets::Assets;
use crate::{problems, watch};

/// The default keymap for this platform.
pub const DEFAULT_KEYMAP: &str = if cfg!(target_os = "macos") {
    "keymaps/default-macos.json"
} else {
    "keymaps/default-linux.json"
};

/// Where a binding comes from; the lower, the stronger.
const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
const DEFAULTS: KeyBindingMetaIndex = KeyBindingMetaIndex(1);

/// Where the user's keymap lives: `$XDG_CONFIG_HOME/ainet/keymap.json`.
pub fn keymap_file() -> PathBuf {
    ainet_core::paths::config_dir().join("keymap.json")
}

/// What a new keymap file starts as when opened from the app.
pub const TEMPLATE: &str = r#"// Your key bindings, on top of ainet's defaults, in Zed's keymap format.
// Changes apply as soon as you save. `null` unbinds a default.
[
  // {
  //   "context": "Workspace",
  //   "bindings": { "ctrl-shift-b": "workspace::ToggleRightDock" }
  // },
]
"#;

/// One section of a keymap file.
#[derive(Debug, Default, Deserialize)]
pub struct Section {
    #[serde(default)]
    pub context: String,
    /// Keystrokes to actions, in file order: later bindings win ties.
    #[serde(default)]
    pub bindings: Bindings,
}

/// A keymap's `bindings` object, in the order it was written.
#[derive(Debug, Default)]
pub struct Bindings(pub Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Bindings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Bindings;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object from keystrokes to actions")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Bindings, A::Error> {
                let mut v = Vec::new();
                while let Some(entry) = map.next_entry::<String, Value>()? {
                    v.push(entry);
                }
                Ok(Bindings(v))
            }
        }
        d.deserialize_map(Ordered)
    }
}

/// Reads a keymap file's sections. An empty file has none.
pub fn parse(text: &str) -> Result<Vec<Section>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json_lenient::from_str(text).map_err(|e| e.to_string())
}

/// An action as a keymap writes it: its name and input, or `None` for
/// `null` (unbind).
pub fn action_of(value: &Value) -> Result<Option<(&str, Option<&Value>)>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(name) => Ok(Some((name, None))),
        Value::Array(items) => match items.as_slice() {
            [Value::String(name), input] => Ok(Some((name, Some(input)))),
            _ => Err(format!("expected [name, input], found {value}")),
        },
        _ => Err(format!(
            "expected an action name, [name, input] or null, found {value}"
        )),
    }
}

/// Turns sections into GPUI key bindings, skipping (and describing) those
/// that name unknown actions, bad keystrokes or bad contexts, the way Zed
/// loads what it can of a broken keymap.
pub fn bindings(sections: &[Section], cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for section in sections {
        let context: Option<Rc<KeyBindingContextPredicate>> = if section.context.is_empty() {
            None
        } else {
            match KeyBindingContextPredicate::parse(&section.context) {
                Ok(p) => Some(p.into()),
                Err(e) => {
                    errors.push(format!("context \"{}\": {e}", section.context));
                    continue;
                }
            }
        };
        for (keys, value) in &section.bindings.0 {
            match binding(keys, value, context.clone(), cx) {
                Ok(b) => out.push(b),
                Err(e) => errors.push(format!("\"{keys}\": {e}")),
            }
        }
    }
    (out, errors)
}

fn binding(
    keys: &str,
    value: &Value,
    context: Option<Rc<KeyBindingContextPredicate>>,
    cx: &App,
) -> Result<KeyBinding, String> {
    let (action, input): (Box<dyn Action>, Option<SharedString>) = match action_of(value)? {
        None => (NoAction.boxed_clone(), None),
        Some((name, input)) => {
            let action = cx.build_action(name, input.cloned()).map_err(|e| e.to_string())?;
            (action, input.map(|i| i.to_string().into()))
        }
    };
    KeyBinding::load(keys, action, context, false, input, cx.keyboard_mapper().as_ref())
        .map_err(|e| e.to_string())
}

/// Follows the user's keymap file.
struct UserKeymap {
    file: Option<PathBuf>,
    _watch: Option<Task<()>>,
}

impl Global for UserKeymap {}

/// Binds the default keymap and the user's (none in tests), and follows the
/// user's file from then on.
pub fn init(file: Option<PathBuf>, cx: &mut App) {
    let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
    load(text.as_deref().unwrap_or(""), cx);
    let watch = file.clone().map(|file| {
        watch::watch(file, cx, |text, cx| {
            load(text.as_deref().unwrap_or(""), cx);
            cx.refresh_windows();
        })
    });
    cx.set_global(UserKeymap { file, _watch: watch });
}

/// Replaces every binding with the defaults plus `user`. What cannot be
/// read is reported and the rest still applies.
pub fn load(user: &str, cx: &mut App) {
    let defaults = Assets::text(DEFAULT_KEYMAP)
        .ok_or_else(|| format!("{DEFAULT_KEYMAP} is missing"))
        .and_then(|text| parse(&text));
    let (default_bindings, default_errors) = match &defaults {
        Ok(sections) => bindings(sections, cx),
        Err(e) => (Vec::new(), vec![format!("{DEFAULT_KEYMAP}: {e}")]),
    };
    // The defaults ship with the app; a broken one is a bug, not the user's.
    if !default_errors.is_empty() {
        problems::set(problems::Source::Startup, Some(default_errors.join("; ")), cx);
    }
    let (user_bindings, user_errors) = match parse(user) {
        Ok(sections) => bindings(&sections, cx),
        Err(e) => (Vec::new(), vec![e]),
    };
    cx.clear_key_bindings();
    // As in Zed, a default `null` hides other defaults but not the user's.
    cx.bind_keys(default_bindings.into_iter().map(|b| b.with_meta(DEFAULTS)));
    cx.bind_keys(user_bindings.into_iter().map(|b| b.with_meta(USER)));
    let problem = (!user_errors.is_empty()).then(|| user_errors.join("; "));
    problems::set(problems::Source::Keymap, problem, cx);
}

/// The user's keymap file, if it is being followed.
pub fn user_file(cx: &App) -> Option<PathBuf> {
    cx.try_global::<UserKeymap>().and_then(|k| k.file.clone())
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::actions;

    #[test]
    fn reads_zeds_format() {
        let sections = parse(
            r#"
            // Comments and trailing commas, as in Zed.
            [
              { "bindings": { "ctrl-q": "ainet::Quit", } },
              {
                "context": "FleetPanel",
                "bindings": {
                  "down": "fleet::SelectNext",
                  "up": "fleet::SelectPrev",
                  "ctrl-x": null,
                  "ctrl-k": ["some::Action", { "x": 1 }],
                },
              },
            ]
            "#,
        )
        .unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].context, "");
        let keys: Vec<_> = sections[1].bindings.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["down", "up", "ctrl-x", "ctrl-k"], "in file order");
        let actions: Vec<_> = sections[1]
            .bindings
            .0
            .iter()
            .map(|(_, v)| action_of(v).unwrap())
            .collect();
        assert_eq!(actions[0], Some(("fleet::SelectNext", None)));
        assert_eq!(actions[2], None);
        assert_eq!(actions[3].unwrap().0, "some::Action");
        assert!(action_of(&serde_json::json!(3)).is_err());
        assert!(action_of(&serde_json::json!(["a", 1, 2])).is_err());
        assert!(parse("").unwrap().is_empty());
        assert!(parse(TEMPLATE).unwrap().is_empty());
        assert!(parse("{").is_err());
        assert!(parse(r#"[{ "bindings": [] }]"#).is_err());
    }

    #[gpui::test]
    fn every_default_binding_names_a_real_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for path in ["keymaps/default-linux.json", "keymaps/default-macos.json"] {
                let sections = parse(&Assets::text(path).unwrap()).unwrap();
                for section in &sections {
                    for (_, value) in &section.bindings.0 {
                        if let Some((name, input)) = action_of(value).unwrap() {
                            assert!(cx.build_action(name, input.cloned()).is_ok(), "{path}: {name}");
                        }
                    }
                }
                let contexts = [
                    "",
                    "Workspace",
                    "FleetPanel",
                    "Terminal",
                    "Inbox",
                    "Comms",
                    "QueuePanel",
                    "Map",
                    "Picker",
                    "TextInput",
                    "TextInput && multiline",
                    "CommsCompose > TextInput",
                    "CommsCompose > TextInput && empty",
                ];
                for section in &sections {
                    assert!(
                        contexts.contains(&section.context.as_str()),
                        "{path}: {}",
                        section.context
                    );
                }
                let (bound, errors) = bindings(&sections, cx);
                assert!(errors.is_empty(), "{path}: {errors:?}");
                assert!(!bound.is_empty());
            }
        });
    }

    #[gpui::test]
    fn a_focused_terminal_gets_every_ctrl_letter_and_the_list_keys(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for path in ["keymaps/default-linux.json", "keymaps/default-macos.json"] {
                let (bound, _) = bindings(&parse(&Assets::text(path).unwrap()).unwrap(), cx);
                let keymap = gpui::Keymap::new(bound);
                let stack = ["Workspace", "Terminal"].map(|c| gpui::KeyContext::parse(c).unwrap());
                let ctrl = ('a'..='z')
                    .map(String::from)
                    .chain(["\\".into()])
                    .map(|k| format!("ctrl-{k}"));
                let plain = [
                    "escape",
                    "enter",
                    "tab",
                    "shift-tab",
                    "backspace",
                    "up",
                    "down",
                    "left",
                    "right",
                    "home",
                    "end",
                    "pageup",
                    "pagedown",
                    "delete",
                    "a",
                    "j",
                    "space",
                ]
                .map(String::from);
                for keys in ctrl.chain(plain) {
                    let keystroke = gpui::Keystroke::parse(&keys).unwrap();
                    let (found, _) = keymap.bindings_for_input(&[keystroke], &stack);
                    let names: Vec<_> = found.iter().map(|b| b.action().name()).collect();
                    assert!(names.is_empty(), "{path}: {keys} is bound to {names:?}");
                }
            }
        });
    }

    #[gpui::test]
    fn the_user_keymap_overrides_and_reports_what_it_cannot_read(cx: &mut TestAppContext) {
        cx.update(|cx| {
            load("", cx);
            let quit = |cx: &App| {
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&actions::Quit)
                    .map(|b| b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
            };
            let default_quit = if cfg!(target_os = "macos") { "cmd-q" } else { "ctrl-q" };
            assert_eq!(quit(cx), [default_quit]);
            assert!(problems::lines(cx).is_empty());

            load(
                r#"[{ "bindings": { "ctrl-alt-q": "ainet::Quit", "ctrl-1": "no::SuchAction", "bad-key-": "ainet::Quit" } }]"#,
                cx,
            );
            assert_eq!(quit(cx), [default_quit, "ctrl-alt-q"]);
            let problems = problems::lines(cx).join("\n");
            assert!(problems.contains("no::SuchAction"), "{problems}");
            assert!(problems.contains("bad-key-"), "{problems}");

            // A broken file keeps the defaults, and the problem shows.
            load("[{", cx);
            assert_eq!(quit(cx), [default_quit]);
            assert_eq!(problems::lines(cx).len(), 1);

            // Fixed: the problem goes.
            load("[]", cx);
            assert!(problems::lines(cx).is_empty());
        });
    }
}
