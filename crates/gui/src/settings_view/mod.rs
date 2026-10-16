//! The Settings view (`workspace::OpenSettings`), after Zed's settings
//! window: a search box over a list of categories on the left, the fields
//! of the one picked on the right. Each field has its description, its
//! default, a dot when it differs from it, a reset, and when a change
//! applies; what goes wrong with a change shows under it.
//!
//! The app's own settings (Appearance, Terminal, Workspace) are written
//! to settings.json here, as the theme selector writes the theme: only
//! that value changes in the text, and the app takes it at once. ainet's
//! (Daemon … Cost) go through ainetd when it runs (`SettingsSet`, which
//! edits the same file the same way and puts it in force), else into the
//! file directly. The Keymap category is its own editor
//! ([`keymap_editor`]); settings.json and keymap.json stay one action away
//! (`workspace::OpenSettingsFile`, `OpenKeymapFile`).

mod controls;
pub mod field;
pub mod keymap_editor;
pub mod pages;
#[cfg(all(test, not(target_os = "macos")))]
mod tests;

use std::collections::HashMap;

use ainet_core::Request;
use ainet_core::settings::SettingsReport;
use ainet_model::ConfigReply;
use gpui::{Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle, Subscription};
use serde_json::Value;
use ui::prelude::*;

use crate::fleet::{Connection, FleetModel};
use crate::item::{Item, ItemEvent};
use crate::persistence::SavedItem;
use crate::settings::{self, AppSettings};
use crate::text_input::{InputEvent, TextInput};
use field::{Field, Kind, Owner, at, same};
use keymap_editor::KeymapEditor;
use pages::Page;

/// Which of a field's inputs: its value, or the new item of a list or
/// entry of a map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Value,
    NewItem,
    NewKey,
    NewValue,
}

pub struct SettingsItem {
    fleet: Entity<FleetModel>,
    focus: FocusHandle,
    search: Entity<TextInput>,
    page: Page,
    fields: Vec<(Page, Field)>,
    /// The inputs of the fields that are typed into.
    inputs: HashMap<(&'static str, Slot), Entity<TextInput>>,
    /// The text each value input was last given from its setting: an
    /// input still showing it has not been edited, and follows the
    /// setting.
    synced: HashMap<&'static str, String>,
    /// Why the last change of a field did not happen, by key.
    errors: HashMap<&'static str, String>,
    /// ainet's settings as ainetd has them, or the file without it.
    report: Option<SettingsReport>,
    /// ainet's defaults, until a report comes.
    daemon_defaults: Value,
    keymap: Entity<KeymapEditor>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsItem {
    pub fn new(
        fleet: Entity<FleetModel>,
        page: Option<Page>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_fields(fleet, page, pages::fields(), window, cx)
    }

    /// The view over `fields` (tests add their own).
    pub fn with_fields(
        fleet: Entity<FleetModel>,
        page: Option<Page>,
        fields: Vec<(Page, Field)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let search = cx.new(|cx| TextInput::new(window, cx).with_placeholder("Search settings…"));
        let mut subscriptions = vec![
            cx.subscribe_in(&search, window, |this, search, event: &InputEvent, _, cx| {
                if *event == InputEvent::Cancelled && !search.read(cx).is_empty() {
                    search.update(cx, |s, cx| s.set_text("", cx));
                }
                if *event != InputEvent::Blurred {
                    this.scroll.set_offset(Default::default());
                    cx.notify();
                }
            }),
            // The view's own keyboard goes on to the search box.
            cx.on_focus(&focus, window, |this, window, cx| {
                window.focus(&this.search.focus_handle(cx), cx);
            }),
            cx.observe_global_in::<AppSettings>(window, |this, _, cx| {
                this.sync_inputs(cx);
                cx.notify();
            }),
        ];
        let mut inputs = HashMap::new();
        for (_, field) in &fields {
            let slots: &[Slot] = match field.kind {
                Kind::Number { .. } | Kind::Text { .. } | Kind::Path { .. } => &[Slot::Value],
                Kind::List { .. } => &[Slot::NewItem],
                Kind::Map { .. } => &[Slot::NewKey, Slot::NewValue],
                Kind::Toggle | Kind::Dropdown(_) | Kind::Theme => &[],
            };
            for &slot in slots {
                let placeholder = match (&field.kind, slot) {
                    (Kind::List { placeholder }, _) => *placeholder,
                    (Kind::Map { key_placeholder, .. }, Slot::NewKey) => *key_placeholder,
                    (
                        Kind::Map {
                            value_placeholder, ..
                        },
                        _,
                    ) => *value_placeholder,
                    _ => "",
                };
                let input = cx.new(|cx| TextInput::new(window, cx).with_placeholder(placeholder));
                let key = field.key;
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        this.on_input(key, slot, *event, window, cx)
                    },
                ));
                inputs.insert((key, slot), input);
            }
        }
        let keymap = cx.new(|cx| KeymapEditor::new(window, cx));
        let mut this = SettingsItem {
            fleet,
            focus,
            search,
            page: page.unwrap_or(Page::Appearance),
            fields,
            inputs,
            synced: HashMap::new(),
            errors: HashMap::new(),
            report: None,
            daemon_defaults: serde_json::to_value(ainet_core::settings::Settings::default())
                .unwrap_or(Value::Null),
            keymap,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.sync_inputs(cx);
        this.refresh(cx);
        this
    }

    /// The category on show.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn page(&self) -> Page {
        self.page
    }

    /// Shows `page`, the search cleared.
    pub fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.scroll.set_offset(Default::default());
        cx.notify();
    }

    #[cfg(test)]
    pub fn search(&self) -> &Entity<TextInput> {
        &self.search
    }

    #[cfg(test)]
    pub fn keymap(&self) -> &Entity<KeymapEditor> {
        &self.keymap
    }

    #[cfg(test)]
    fn input(&self, key: &str, slot: Slot) -> Entity<TextInput> {
        self.inputs
            .iter()
            .find(|((k, s), _)| *k == key && *s == slot)
            .map(|(_, input)| input.clone())
            .unwrap_or_else(|| panic!("no input {key} {slot:?}"))
    }

    #[cfg(test)]
    fn error(&self, key: &str) -> Option<&str> {
        self.errors.get(key).map(String::as_str)
    }

    /// The fields shown: the search's matches across every category, or
    /// the category's.
    fn shown(&self, cx: &App) -> Vec<(Page, &Field)> {
        let query = self.search.read(cx).text().trim().to_string();
        self.fields
            .iter()
            .filter(|(page, field)| {
                let visible = match field.part_of {
                    Some((parent, when)) => when(&self.value_at(field.owner, parent, cx)),
                    None => true,
                };
                visible
                    && if query.is_empty() {
                        *page == self.page
                    } else {
                        field.matches(&query)
                    }
            })
            .map(|(page, field)| (*page, field))
            .collect()
    }

    /// How many fields of `page` match the search.
    fn matches_in(&self, page: Page, query: &str) -> usize {
        self.fields
            .iter()
            .filter(|(p, f)| *p == page && f.part_of.is_none() && f.matches(query))
            .count()
    }

    fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().map(|(_, f)| f).find(|f| f.key == key)
    }

    /// A field's value in force: `Null` when unset.
    fn value(&self, field: &Field, cx: &App) -> Value {
        self.value_at(field.owner, field.key, cx)
    }

    fn value_at(&self, owner: Owner, key: &str, cx: &App) -> Value {
        match owner {
            Owner::App => at(&serde_json::to_value(settings::get(cx)).unwrap_or_default(), key),
            Owner::Daemon => match self.report.as_ref().and_then(|r| r.get(key)) {
                Some(e) if e.key == key => e.value.clone(),
                Some(e) => at(&e.value, &key[e.key.len() + 1..]),
                None => at(&self.daemon_defaults, key),
            },
        }
    }

    /// A field's default.
    fn default(&self, field: &Field) -> Value {
        match field.owner {
            Owner::App => at(
                &serde_json::to_value(settings::Settings::default()).unwrap_or_default(),
                field.key,
            ),
            Owner::Daemon => at(&self.daemon_defaults, field.key),
        }
    }

    /// Whether ainetd is there to ask (the demo answers too).
    fn connected(&self, cx: &App) -> bool {
        matches!(
            self.fleet.read(cx).connection(),
            Connection::Connected | Connection::Demo
        )
    }

    /// Asks again for ainet's settings (and the key bindings): from
    /// ainetd, or the file when it is not there.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.keymap.update(cx, |k, cx| k.reload(cx));
        if !self.connected(cx) {
            self.report = Some(local_report(cx));
            self.sync_inputs(cx);
            return;
        }
        let this = cx.weak_entity();
        self.fleet.update(cx, |fleet, cx| {
            fleet.config(
                Request::SettingsGet { key: None },
                move |reply, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.report = Some(match reply {
                            Ok(ConfigReply::Settings(report)) => *report,
                            // Not there after all: as the file says.
                            _ => local_report(cx),
                        });
                        this.sync_inputs(cx);
                        cx.notify();
                    });
                },
                cx,
            )
        });
    }

    /// Puts each setting's value in its input, unless the input was typed
    /// into since.
    fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<&'static str> = self.fields.iter().map(|(_, f)| f.key).collect();
        for key in keys {
            self.sync_input(key, false, cx);
        }
    }

    /// Puts `key`'s value in its input: always with `force` (it was just
    /// written), else only if the input still shows what it was given.
    fn sync_input(&mut self, key: &'static str, force: bool, cx: &mut Context<Self>) {
        let Some(field) = self.field(key).cloned() else {
            return;
        };
        let Some(input) = self.inputs.get(&(key, Slot::Value)).cloned() else {
            return;
        };
        let value = self.value(&field, cx);
        let text = field.input_text(&value);
        let placeholder = self.unset_words(&field, cx);
        let shown = input.read(cx).text().to_string();
        let given = self.synced.get(key).cloned().unwrap_or_default();
        input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, cx);
            if force || shown == given {
                input.set_text(&text, cx);
            }
        });
        self.synced.insert(key, text);
    }

    /// What an unset field means, in words: its own words for it, or its
    /// default.
    fn unset_words(&self, field: &Field, cx: &App) -> String {
        if let Some(words) = field.when_unset {
            return words(settings::get(cx));
        }
        match &field.kind {
            Kind::Text { placeholder } if !placeholder.is_empty() => placeholder.to_string(),
            _ => match self.default(field) {
                Value::Null => "unset".into(),
                d => field.show(&d),
            },
        }
    }

    fn on_input(
        &mut self,
        key: &'static str,
        slot: Slot,
        event: InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (slot, event) {
            (Slot::Value, InputEvent::Submitted | InputEvent::Blurred) => self.commit(key, cx),
            (Slot::Value, InputEvent::Cancelled) => {
                self.errors.remove(key);
                self.sync_input(key, true, cx);
                cx.notify();
            }
            (Slot::NewItem, InputEvent::Submitted) => self.add_item(key, cx),
            (Slot::NewKey, InputEvent::Submitted) => {
                if let Some(next) = self.inputs.get(&(key, Slot::NewValue)) {
                    window.focus(&next.focus_handle(cx), cx);
                }
            }
            (Slot::NewValue, InputEvent::Submitted) => self.add_entry(key, cx),
            _ => {}
        }
    }

    /// Writes what was typed in `key`'s input, if it changed.
    fn commit(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let (Some(field), Some(input)) = (self.field(key).cloned(), self.inputs.get(&(key, Slot::Value)))
        else {
            return;
        };
        let text = input.read(cx).text().to_string();
        if self.synced.get(key) == Some(&text) {
            return;
        }
        match field.parse_input(&text) {
            Ok(value) => self.write(key, value, cx),
            Err(why) => {
                self.errors.insert(key, why);
                cx.notify();
            }
        }
    }

    /// Adds what was typed to `key`'s list.
    fn add_item(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(field) = self.field(key).cloned() else {
            return;
        };
        let text = self.input_text(key, Slot::NewItem, cx);
        if text.is_empty() {
            return;
        }
        let mut items = match self.value(&field, cx) {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        items.push(Value::String(text));
        self.write(key, Some(Value::Array(items)), cx);
    }

    /// Adds the name and value typed to `key`'s map.
    fn add_entry(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(field) = self.field(key).cloned() else {
            return;
        };
        let (name, value) = (
            self.input_text(key, Slot::NewKey, cx),
            self.input_text(key, Slot::NewValue, cx),
        );
        if name.is_empty() {
            self.errors.insert(key, "a name first".into());
            return cx.notify();
        }
        let mut map = match self.value(&field, cx) {
            Value::Object(map) => map,
            _ => Default::default(),
        };
        map.insert(name, Value::String(value));
        self.write(key, Some(Value::Object(map)), cx);
    }

    /// Takes item or entry `at` out of `key`'s list or map.
    fn remove_at(&mut self, key: &'static str, at: usize, cx: &mut Context<Self>) {
        let Some(field) = self.field(key).cloned() else {
            return;
        };
        let value = match self.value(&field, cx) {
            Value::Array(mut items) if at < items.len() => {
                items.remove(at);
                Value::Array(items)
            }
            Value::Object(mut map) => {
                let Some(name) = map.keys().nth(at).cloned() else {
                    return;
                };
                map.remove(&name);
                Value::Object(map)
            }
            _ => return,
        };
        self.write(key, Some(value), cx);
    }

    fn input_text(&self, key: &'static str, slot: Slot, cx: &App) -> String {
        self.inputs
            .get(&(key, slot))
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default()
    }

    /// Sets `key` to `value`, or unsets it with `None` (so does a value
    /// equal to the default: the file keeps only what you changed). The
    /// app's settings are written here; ainet's through ainetd, else into
    /// the file.
    pub fn write(&mut self, key: &'static str, value: Option<Value>, cx: &mut Context<Self>) {
        let Some(field) = self.field(key).cloned() else {
            return;
        };
        let default = self.default(&field);
        let value = value.filter(|v| field.part_of.is_some() || !same(v, &default));
        match field.owner {
            Owner::App => {
                let result = settings::save_value(&field.path(), value.as_ref(), cx);
                self.written(key, result.map(|_| None), cx);
            }
            Owner::Daemon if self.connected(cx) => {
                let req = match value {
                    Some(value) => Request::SettingsSet {
                        key: key.to_string(),
                        value,
                    },
                    None => Request::SettingsUnset { key: key.to_string() },
                };
                let this = cx.weak_entity();
                self.fleet.update(cx, |fleet, cx| {
                    fleet.config(
                        req,
                        move |reply, cx| {
                            let _ = this.update(cx, |this, cx| {
                                let result = match reply {
                                    Ok(ConfigReply::Settings(report)) => Ok(Some(*report)),
                                    Ok(_) => Err("ainetd answered with something else".into()),
                                    Err(why) => Err(why),
                                };
                                this.written(key, result, cx);
                            });
                        },
                        cx,
                    )
                });
            }
            Owner::Daemon => {
                let result = match settings::file(cx) {
                    Some(file) => match &value {
                        Some(value) => ainet_core::settings::set_in_file(&file, key, value),
                        None => ainet_core::settings::unset_in_file(&file, key),
                    }
                    .map(|_| Some(SettingsReport::read(&file))),
                    None => Err("no settings file to write to".into()),
                };
                self.written(key, result, cx);
            }
        }
    }

    /// A change of `key` is done (with ainet's settings after it), or why
    /// not.
    fn written(
        &mut self,
        key: &'static str,
        result: Result<Option<SettingsReport>, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(report) => {
                if let Some(report) = report {
                    self.report = Some(report);
                }
                self.errors.remove(key);
                for slot in [Slot::NewItem, Slot::NewKey, Slot::NewValue] {
                    if let Some(input) = self.inputs.get(&(key, slot)) {
                        input.update(cx, |i, cx| i.set_text("", cx));
                    }
                }
                self.sync_input(key, true, cx);
            }
            Err(why) => {
                self.errors.insert(key, why);
            }
        }
        cx.notify();
    }
}

/// ainet's settings as settings.json says, with this process's
/// environment, for when ainetd is not there to ask.
fn local_report(cx: &App) -> SettingsReport {
    match settings::file(cx) {
        Some(file) => SettingsReport::read(&file),
        None => ainet_core::settings::parse("")
            .map(|loaded| SettingsReport::new(&loaded, None, &[], None))
            .unwrap_or_default(),
    }
}

impl EventEmitter<ItemEvent> for SettingsItem {}

impl Focusable for SettingsItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for SettingsItem {
    fn tab_content_text(&self, _: &App) -> SharedString {
        "Settings".into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<AnyElement> {
        Some(
            Icon::new(IconName::Settings)
                .size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element(),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some("The app's and ainet's settings".into())
    }

    fn activated(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // The file may have changed by hand meanwhile.
        self.refresh(cx);
    }

    fn saved(&self, _: &App) -> Option<SavedItem> {
        Some(SavedItem::Settings {
            page: Some(self.page.name().to_string()),
        })
    }
}
