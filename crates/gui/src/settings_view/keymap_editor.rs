//! The Settings view's Keymap category, after Zed's keymap editor: every
//! binding by context (the defaults and yours), filtered as you type, and
//! changed by pressing the new keys. Changes go into keymap.json through
//! `keymap_edit` (comments kept) and apply at once. Changing a default's
//! keys or removing it writes `null` for it, as Zed does, so a reset
//! (taking your line out) brings it back.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    Entity, FocusHandle, Focusable, KeybindingKeystroke, Keystroke, KeystrokeEvent, Subscription,
    UniformListScrollHandle, uniform_list,
};
use serde_json::Value;
use ui::{KeyBinding, Tooltip, prelude::*};

use crate::assets::Assets;
use crate::command_palette::humanize;
use crate::keymap::{self, DEFAULT_KEYMAP, Section};
use crate::keymap_edit;
use crate::text_input::{InputEvent, TextInput};

/// Every row is this tall (the list is virtual).
const ROW_HEIGHT: Pixels = px(32.);
/// A binding has at most this many keystrokes (a chord of two).
const MAX_KEYSTROKES: usize = 2;

/// Where a binding comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    /// keymap.json: added, changed, or (with no action) a default unbound.
    User,
}

/// A binding as the table lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The context it applies in; empty for everywhere.
    pub context: String,
    /// Keystrokes, a chord's separated by spaces: `ctrl-alt-k`.
    pub keys: String,
    /// The action's name; `None` for keys unbound.
    pub action: Option<String>,
    /// The action's input, for actions that take one.
    pub input: Option<Value>,
    pub source: Source,
    /// For yours: the default action of the same keys and context, which
    /// yours replaces (or unbinds).
    pub replaces: Option<String>,
}

impl Row {
    /// The action as keymap.json writes it.
    fn action_json(&self) -> Value {
        match (&self.action, &self.input) {
            (Some(name), Some(input)) => Value::Array(vec![Value::String(name.clone()), input.clone()]),
            (Some(name), None) => Value::String(name.clone()),
            (None, _) => Value::Null,
        }
    }

    /// The action in words: `workspace: open map`.
    pub fn action_words(&self) -> String {
        match (&self.action, &self.replaces) {
            (Some(name), _) => humanize(name),
            (None, Some(default)) => format!("{} (unbound)", humanize(default)),
            (None, None) => "unbound".into(),
        }
    }

    /// Whether every word of `query` is in its action, keys, context or
    /// source (any case).
    pub fn matches(&self, query: &str) -> bool {
        let source = match self.source {
            Source::Default => "default",
            Source::User => "user yours",
        };
        let haystack = format!(
            "{} {} {} {} {source}",
            self.action_words(),
            self.action
                .as_deref()
                .or(self.replaces.as_deref())
                .unwrap_or_default(),
            self.keys,
            self.context
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

/// The table's rows: yours, and the defaults you have not replaced, by
/// context, then action, then keys. A later binding of the same keys in
/// a context wins, as when they are loaded.
pub fn rows(defaults: &[Section], user: &[Section]) -> Vec<Row> {
    let flatten = |sections: &[Section]| {
        let mut out: Vec<(String, String, Option<String>, Option<Value>)> = Vec::new();
        for section in sections {
            for (keys, value) in &section.bindings.0 {
                let Ok(action) = keymap::action_of(value) else {
                    continue;
                };
                let (action, input) = match action {
                    Some((name, input)) => (Some(name.to_string()), input.cloned()),
                    None => (None, None),
                };
                out.retain(|(c, k, _, _)| !(c == &section.context && k == keys));
                out.push((section.context.clone(), keys.clone(), action, input));
            }
        }
        out
    };
    let defaults = flatten(defaults);
    let user = flatten(user);
    let default_of: HashMap<(&str, &str), Option<&String>> = defaults
        .iter()
        .map(|(c, k, a, _)| ((c.as_str(), k.as_str()), a.as_ref()))
        .collect();
    let mut out: Vec<Row> = user
        .iter()
        .map(|(context, keys, action, input)| Row {
            context: context.clone(),
            keys: keys.clone(),
            action: action.clone(),
            input: input.clone(),
            source: Source::User,
            replaces: default_of
                .get(&(context.as_str(), keys.as_str()))
                .copied()
                .flatten()
                .cloned(),
        })
        .collect();
    out.extend(
        defaults
            .iter()
            .filter(|(c, k, a, _)| a.is_some() && !user.iter().any(|(uc, uk, _, _)| uc == c && uk == k))
            .map(|(context, keys, action, input)| Row {
                context: context.clone(),
                keys: keys.clone(),
                action: action.clone(),
                input: input.clone(),
                source: Source::Default,
                replaces: None,
            }),
    );
    out.sort_by(|a, b| {
        (&a.context, a.action.as_ref().or(a.replaces.as_ref()), &a.keys).cmp(&(
            &b.context,
            b.action.as_ref().or(b.replaces.as_ref()),
            &b.keys,
        ))
    });
    out
}

/// New keys being pressed for a row.
struct Recording {
    /// The row, in `rows`.
    row: usize,
    /// Keep the row's keys and add these (else they replace them).
    add: bool,
    keys: Vec<Keystroke>,
    /// Keys are caught app-wide; only the editor's window's are recorded.
    _intercept: Subscription,
    /// Recording stops once the editor loses focus: clicked away, the
    /// keys typed elsewhere must reach where they are typed.
    _blur: Subscription,
}

pub struct KeymapEditor {
    focus: FocusHandle,
    filter: Entity<TextInput>,
    rows: Vec<Row>,
    /// The rows through the filter.
    shown: Vec<usize>,
    recording: Option<Recording>,
    /// Why the last change did not happen, or what is wrong with
    /// keymap.json.
    error: Option<String>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl KeymapEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| TextInput::new(window, cx).with_placeholder("Filter by action, keys or context…"));
        let subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this, filter, event: &InputEvent, _, cx| match event {
                InputEvent::Edited => this.refilter(cx),
                InputEvent::Cancelled if !filter.read(cx).is_empty() => {
                    filter.update(cx, |f, cx| f.set_text("", cx));
                }
                _ => {}
            },
        )];
        let mut this = KeymapEditor {
            focus: cx.focus_handle(),
            filter,
            rows: Vec::new(),
            shown: Vec::new(),
            recording: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// The rows through the filter, in order.
    #[cfg(test)]
    pub fn shown_rows(&self) -> Vec<&Row> {
        self.shown.iter().map(|&ix| &self.rows[ix]).collect()
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Reads the bindings again: the defaults and keymap.json.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let defaults = Assets::text(DEFAULT_KEYMAP)
            .and_then(|text| keymap::parse(&text).ok())
            .unwrap_or_default();
        let (user, error) = match self.user_text(cx) {
            Ok(text) => match keymap::parse(&text) {
                Ok(sections) => (sections, None),
                Err(e) => (Vec::new(), Some(format!("keymap.json does not read: {e}"))),
            },
            Err(e) => (Vec::new(), Some(e)),
        };
        self.rows = rows(&defaults, &user);
        self.error = error;
        self.recording = None;
        self.refilter(cx);
    }

    /// How many bindings `query` matches (the settings search says so).
    pub fn count_matching(&self, query: &str) -> usize {
        self.rows.iter().filter(|r| r.matches(query)).count()
    }

    /// Filters by `text`, and gives the filter the keyboard.
    #[cfg(test)]
    pub fn filter_text(&self, cx: &App) -> String {
        self.filter.read(cx).text().to_string()
    }

    pub fn set_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| {
            f.set_text(text, cx);
            f.move_to_end(cx);
        });
        window.focus(&self.filter.focus_handle(cx), cx);
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).text().to_string();
        self.shown = (0..self.rows.len())
            .filter(|&ix| self.rows[ix].matches(&query))
            .collect();
        cx.notify();
    }

    /// keymap.json's text, empty while there is none.
    fn user_text(&self, cx: &App) -> Result<String, String> {
        let Some(file) = keymap::user_file(cx) else {
            return Ok(String::new());
        };
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("cannot read {}: {e}", file.display())),
        }
    }

    /// Edits keymap.json with `edit`, saves it and binds what it says.
    fn edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&str) -> Result<String, String>) {
        let result = keymap::user_file(cx)
            .ok_or_else(|| "no keymap.json to write to".to_string())
            .and_then(|file| {
                let text = self.user_text(cx)?;
                let edited = edit(&text)?;
                crate::settings::write_whole(&file, &edited)
                    .map_err(|e| format!("cannot save {}: {e}", file.display()))?;
                Ok(edited)
            });
        match result {
            Ok(edited) => {
                keymap::load(&edited, cx);
                self.reload(cx);
            }
            Err(why) => {
                self.error = Some(why);
                cx.notify();
            }
        }
    }

    /// Starts listening for new keys for row `row` (in `rows`); with
    /// `add`, they are bound besides its own.
    pub fn record(&mut self, row: usize, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let ours = window.window_handle();
        // Before the keymap sees them: while recording, keys bound to
        // something are keys like any other.
        let intercept = cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            if window.window_handle() == ours {
                let _ = this.update(cx, |this, cx| this.pressed(&event.keystroke, window, cx));
            }
        });
        // Away from any text field, so typed letters go nowhere else.
        window.focus(&self.focus, cx);
        let blur = cx.on_blur(&self.focus, window, |this, _, cx| {
            if this.recording.take().is_some() {
                cx.notify();
            }
        });
        self.recording = Some(Recording {
            row,
            add,
            keys: Vec::new(),
            _intercept: intercept,
            _blur: blur,
        });
        cx.notify();
    }

    /// A key pressed while recording: escape gives up, enter saves the
    /// keys so far, others are added (a third starts again).
    fn pressed(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        cx.stop_propagation();
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => self.stop_recording(window, cx),
            "enter" if plain && !recording.keys.is_empty() => self.save_recording(window, cx),
            _ => {
                if recording.keys.len() >= MAX_KEYSTROKES {
                    recording.keys.clear();
                }
                recording.keys.push(keystroke.clone());
                cx.notify();
            }
        }
    }

    #[cfg(test)]
    pub fn recorded(&self) -> Vec<String> {
        self.recording
            .as_ref()
            .map(|r| r.keys.iter().map(Keystroke::unparse).collect())
            .unwrap_or_default()
    }

    pub fn stop_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Binds the keys recorded to the row's action, in its context.
    pub fn save_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        let Some(row) = self.rows.get(recording.row).cloned() else {
            return;
        };
        let keys = recording
            .keys
            .iter()
            .map(Keystroke::unparse)
            .collect::<Vec<_>>()
            .join(" ");
        if keys.is_empty() || (keys == row.keys && !recording.add) {
            return cx.notify();
        }
        let action = row.action_json();
        self.edit(cx, |text| {
            let mut text = text.to_string();
            if !recording.add {
                text = match row.source {
                    // The default's old keys stop doing it.
                    Source::Default => keymap_edit::bind(&text, &row.context, &row.keys, &Value::Null)?,
                    Source::User => keymap_edit::unbind(&text, &row.context, &row.keys)?,
                };
            }
            keymap_edit::bind(&text, &row.context, &keys, &action)
        });
    }

    /// Takes the row's binding away: yours from keymap.json, a default
    /// (or yours over one) unbound with `null`.
    pub fn remove(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(row).cloned() else {
            return;
        };
        self.edit(cx, |text| match (row.source, &row.replaces) {
            (Source::User, None) => keymap_edit::unbind(text, &row.context, &row.keys),
            _ => keymap_edit::bind(text, &row.context, &row.keys, &Value::Null),
        });
    }

    /// Takes your line for the row's keys out of keymap.json: the default,
    /// if any, applies again.
    pub fn reset(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(row).cloned() else {
            return;
        };
        self.edit(cx, |text| keymap_edit::unbind(text, &row.context, &row.keys));
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(&at) = self.shown.get(ix) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let row = &self.rows[at];
        let colors = cx.theme().colors();
        let recording_here = self.recording.as_ref().is_some_and(|r| r.row == at);
        let source = match (row.source, &row.action) {
            (Source::Default, _) => ("default", Color::Muted),
            (Source::User, None) => ("unbound", Color::Warning),
            (Source::User, Some(_)) if row.replaces.is_some() => ("yours, over a default", Color::Accent),
            (Source::User, Some(_)) => ("yours", Color::Accent),
        };
        let icon_button = |id: &str, icon: IconName, tip: &'static str| {
            IconButton::new(SharedString::from(format!("keymap-{id}-{ix}")), icon)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
                .tooltip(Tooltip::text(tip))
        };
        let keys: AnyElement = if recording_here {
            let pressed = self.recording.as_ref().map(|r| &r.keys[..]).unwrap_or_default();
            let text = if pressed.is_empty() {
                "press keys…".to_string()
            } else {
                pressed
                    .iter()
                    .map(Keystroke::unparse)
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            div()
                .debug_selector(|| "KEYMAP-RECORDING".into())
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(colors.border_focused)
                .child(Label::new(text).size(LabelSize::Small).buffer_font(cx))
                .into_any_element()
        } else {
            keys_element(&row.keys, cx)
        };
        h_flex()
            .id(("keymap-row", ix))
            .debug_selector(move || format!("KEYMAP-ROW-{ix}"))
            .h(ROW_HEIGHT)
            .w_full()
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(colors.border_variant)
            .when(recording_here, |el| el.bg(colors.element_selected))
            .child(
                div().w(relative(0.34)).min_w_0().child(
                    Label::new(row.action_words())
                        .size(LabelSize::Small)
                        .color(if row.action.is_some() {
                            Color::Default
                        } else {
                            Color::Muted
                        })
                        .truncate(),
                ),
            )
            .child(div().w(relative(0.2)).min_w_0().child(keys))
            .child(
                div().w(relative(0.2)).min_w_0().child(
                    Label::new(
                        if row.context.is_empty() {
                            "anywhere"
                        } else {
                            row.context.as_str()
                        }
                        .to_string(),
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .buffer_font(cx)
                    .truncate(),
                ),
            )
            .child(
                div().w(relative(0.12)).min_w_0().child(
                    Label::new(source.0)
                        .size(LabelSize::XSmall)
                        .color(source.1)
                        .truncate(),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .justify_end()
                    .gap_0p5()
                    .when(row.action.is_some(), |el| {
                        el.child(
                            div().debug_selector(move || format!("KEYMAP-RECORD-{ix}")).child(
                                icon_button("record", IconName::Keyboard, "Change Keys: press the new ones")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.record(at, false, window, cx)
                                    })),
                            ),
                        )
                        .child(div().debug_selector(move || format!("KEYMAP-ADD-{ix}")).child(
                            icon_button("add", IconName::Plus, "Add Keys for This Action").on_click(
                                cx.listener(move |this, _, window, cx| this.record(at, true, window, cx)),
                            ),
                        ))
                        .child(
                            div().debug_selector(move || format!("KEYMAP-REMOVE-{ix}")).child(
                                icon_button("remove", IconName::Trash, "Remove")
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove(at, cx))),
                            ),
                        )
                    })
                    .when(row.source == Source::User, |el| {
                        el.child(
                            div().debug_selector(move || format!("KEYMAP-RESET-{ix}")).child(
                                icon_button("reset", IconName::Undo, "Reset to Default")
                                    .on_click(cx.listener(move |this, _, _, cx| this.reset(at, cx))),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// Keys as Zed draws them, or as written when they do not parse.
fn keys_element(keys: &str, cx: &App) -> AnyElement {
    let parsed: Option<Vec<KeybindingKeystroke>> = keys
        .split_whitespace()
        .map(|k| Keystroke::parse(k).ok().map(KeybindingKeystroke::from_keystroke))
        .collect();
    match parsed {
        Some(strokes) if !strokes.is_empty() => {
            KeyBinding::from_keystrokes(Rc::from(strokes), false).into_any_element()
        }
        _ => Label::new(keys.to_string())
            .size(LabelSize::Small)
            .buffer_font(cx)
            .into_any_element(),
    }
}

impl Focusable for KeymapEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KeymapEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let header = |text: &'static str, width: f32| {
            div()
                .w(relative(width))
                .child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted))
        };
        let recording = self.recording.as_ref().map(|r| {
            let action = self.rows.get(r.row).map(Row::action_words).unwrap_or_default();
            let verb = if r.add { "More keys" } else { "New keys" };
            h_flex()
                .mx_8()
                .my_1()
                .p_2()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(colors.border_focused)
                .child(
                    Icon::new(IconName::Keyboard)
                        .size(IconSize::Small)
                        .color(Color::Accent),
                )
                .child(
                    Label::new(format!(
                        "{verb} for \"{action}\": press them, then enter to save or escape to give up"
                    ))
                    .size(LabelSize::Small),
                )
                .child(div().flex_1())
                .child(
                    div().debug_selector(|| "KEYMAP-SAVE".into()).child(
                        Button::new("keymap-save", "Save")
                            .label_size(LabelSize::Small)
                            .disabled(r.keys.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.save_recording(window, cx))),
                    ),
                )
                .child(
                    Button::new("keymap-cancel", "Cancel")
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, window, cx| this.stop_recording(window, cx))),
                )
        });
        v_flex()
            .key_context("KeymapEditor")
            .track_focus(&self.focus)
            .size_full()
            .child(
                h_flex()
                    .mx_8()
                    .mb_2()
                    .gap_2()
                    .child(
                        h_flex()
                            .debug_selector(|| "KEYMAP-FILTER".into())
                            .flex_1()
                            .h_7()
                            .px_1p5()
                            .gap_1p5()
                            .rounded_sm()
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                Icon::new(IconName::MagnifyingGlass)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(div().flex_1().min_w_0().child(self.filter.clone())),
                    )
                    .child(
                        Label::new(format!("{} of {} bindings", self.shown.len(), self.rows.len()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .debug_selector(|| "KEYMAP-ERROR".into())
                        .mx_8()
                        .child(Label::new(error).size(LabelSize::Small).color(Color::Error)),
                )
            })
            .children(recording)
            .child(
                h_flex()
                    .mx_8()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(header("Action", 0.34))
                    .child(header("Keys", 0.2))
                    .child(header("Context", 0.2))
                    .child(header("Source", 0.12)),
            )
            .child(
                div().flex_1().min_h_0().mx_8().child(
                    uniform_list(
                        "keymap-rows",
                        self.shown.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range.map(|ix| this.render_row(ix, cx)).collect()
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.scroll),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yours_replace_the_defaults_of_the_same_keys() {
        let defaults = keymap::parse(
            r#"[
              { "bindings": { "ctrl-q": "ainet::Quit" } },
              { "context": "Workspace", "bindings": { "ctrl-t": "workspace::OpenMap", "ctrl-j": "workspace::ToggleBottomDock" } }
            ]"#,
        )
        .unwrap();
        let user = keymap::parse(
            r#"[
              { "context": "Workspace", "bindings": { "ctrl-t": null, "ctrl-alt-m": "workspace::OpenMap" } },
              { "context": "Workspace", "bindings": { "ctrl-j": "workspace::OpenInbox" } }
            ]"#,
        )
        .unwrap();
        let rows = rows(&defaults, &user);
        let line = |r: &Row| {
            format!(
                "{}|{}|{}|{:?}|{:?}",
                r.context,
                r.keys,
                r.action_words(),
                r.source,
                r.replaces
            )
        };
        let lines: Vec<String> = rows.iter().map(line).collect();
        assert_eq!(
            lines,
            [
                "|ctrl-q|ainet: quit|Default|None",
                "Workspace|ctrl-j|workspace: open inbox|User|Some(\"workspace::ToggleBottomDock\")",
                "Workspace|ctrl-alt-m|workspace: open map|User|None",
                "Workspace|ctrl-t|workspace: open map (unbound)|User|Some(\"workspace::OpenMap\")",
            ]
        );
        assert!(rows[2].matches("open map ctrl-alt"));
        assert!(rows[3].matches("yours"));
        assert!(rows[0].matches("default quit"));
        assert!(!rows[0].matches("workspace"));
        assert_eq!(
            rows[1].action_json(),
            Value::String("workspace::OpenInbox".into())
        );
        assert_eq!(rows[3].action_json(), Value::Null);
    }
}
