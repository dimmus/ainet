//! How the Settings view is drawn: the search box and categories on the
//! left, and on the right each field as Zed's settings UI lays one out,
//! its label, description and footnote beside its control.

use gpui::{Div, Entity, PathPromptOptions};
use serde_json::Value;
use ui::{ContextMenu, DropdownMenu, Indicator, ListItem, ListItemSpacing, Switch, Tooltip, prelude::*};

use super::field::{Field, Kind, Owner, chosen, same};
use super::pages::Page;
use super::{SettingsItem, Slot};
use crate::actions::theme_selector;
use crate::actions::{OpenKeymapFile, OpenSettingsFile};
use crate::problems;
use crate::settings::{self, ThemeSelection};
use crate::text_input::TextInput;

/// The categories' width.
const NAV_WIDTH: Pixels = px(220.);

impl Render for SettingsItem {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .key_context("Settings")
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.editor_background)
            .child(self.render_nav(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_page(window, cx)),
            )
    }
}

impl SettingsItem {
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let query = self.search.read(cx).text().trim().to_string();
        let searching = !query.is_empty();
        let entry = |page: Page, cx: &mut Context<Self>| {
            let count = searching.then(|| self.matches_in(page, &query));
            let dim = count == Some(0);
            div()
                .debug_selector(move || format!("SETTINGS-NAV-{}", page.name()))
                .child(
                    ListItem::new(SharedString::from(format!("settings-nav-{}", page.name())))
                        .spacing(ListItemSpacing::Sparse)
                        .rounded()
                        .toggle_state(!searching && page == self.page)
                        .child(Label::new(page.title()).size(LabelSize::Small).color(if dim {
                            Color::Disabled
                        } else {
                            Color::Default
                        }))
                        .end_slot::<Label>(count.filter(|n| *n > 0).map(|n| {
                            Label::new(n.to_string())
                                .size(LabelSize::XSmall)
                                .color(Color::Muted)
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| this.show_page(page, cx))),
                )
        };
        let header = |text: &'static str| {
            div().px_2().pt_3().pb_1().child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
        };
        let (app, ainet): (Vec<Page>, Vec<Page>) = Page::ALL.into_iter().partition(|p| p.is_app());
        v_flex()
            .id("settings-nav")
            .w(NAV_WIDTH)
            .flex_none()
            .h_full()
            .p_2()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .child(self.render_search(cx))
            .child(header("THE APP"))
            .children(app.into_iter().map(|p| entry(p, cx)))
            .child(header("AINET"))
            .children(ainet.into_iter().map(|p| entry(p, cx)))
    }

    fn render_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let searching = !self.search.read(cx).is_empty();
        h_flex()
            .debug_selector(|| "SETTINGS-SEARCH".into())
            .h_7()
            .px_1p5()
            .gap_1p5()
            .rounded_sm()
            .bg(colors.editor_background)
            .border_1()
            .border_color(colors.border)
            .child(
                Icon::new(IconName::MagnifyingGlass)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1().min_w_0().child(self.search.clone()))
            .when(searching, |el| {
                el.child(
                    IconButton::new("settings-search-clear", IconName::Close)
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Clear"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.search.update(cx, |s, cx| s.set_text("", cx));
                            cx.notify();
                        })),
                )
            })
    }

    fn render_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search.read(cx).text().trim().to_string();
        let searching = !query.is_empty();
        if !searching && self.page == Page::Keymap {
            return v_flex()
                .size_full()
                .child(div().px_8().pt_4().child(self.render_header(cx)))
                .child(div().flex_1().min_h_0().child(self.keymap.clone()))
                .into_any_element();
        }
        let shown = self.shown(cx);
        let mut body = v_flex()
            .id("settings-page")
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .size_full()
            .px_8()
            .py_4()
            .gap_1();
        if searching {
            body = body.child(Headline::new("Search Results").size(HeadlineSize::Small));
            if shown.is_empty() {
                body = body.child(
                    Label::new(format!("No setting matches \"{query}\"."))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
            }
            let bindings = self.keymap.read(cx).count_matching(&query);
            if bindings > 0 {
                body = body.child(
                    div().debug_selector(|| "SETTINGS-KEYMAP-MATCHES".into()).child(
                        Button::new(
                            "settings-keymap-matches",
                            format!(
                                "{bindings} key binding{} match: show them in Keymap",
                                if bindings == 1 { "" } else { "s" }
                            ),
                        )
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let query = this.search.read(cx).text().trim().to_string();
                            this.show_page(Page::Keymap, cx);
                            this.keymap.update(cx, |k, cx| k.set_filter(&query, window, cx));
                        })),
                    ),
                );
            }
        } else {
            body = body.child(self.render_header(cx));
        }
        body = body.children(self.render_banners(searching, cx));
        let mut last_page = None;
        for (page, field) in shown {
            if searching && last_page != Some(page) {
                last_page = Some(page);
                body = body.child(
                    div().pt_4().child(
                        Label::new(page.title())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .weight(gpui::FontWeight::SEMIBOLD),
                    ),
                );
            }
            body = body.child(self.render_field(field, window, cx));
        }
        if !searching && !self.fields.iter().any(|(p, _)| *p == self.page) {
            body = body.child(self.render_placeholder(self.page));
        }
        body.into_any_element()
    }

    /// The category's title, where its settings are kept, and the file.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page;
        let (where_kept, file_action): (String, Box<dyn gpui::Action>) = match page {
            Page::Keymap => (
                "Your bindings are kept in keymap.json, on top of ainet's defaults, and apply at once."
                    .into(),
                Box::new(OpenKeymapFile),
            ),
            page if page.is_app() => (
                "The app's own settings, in settings.json. Changes apply at once.".into(),
                Box::new(OpenSettingsFile),
            ),
            page => (
                format!(
                    "ainetd and the console read these, from the \"{}\" section of settings.json.",
                    page.section().unwrap_or("daemon")
                ),
                Box::new(OpenSettingsFile),
            ),
        };
        let file = if page == Page::Keymap {
            "Open keymap.json"
        } else {
            "Open settings.json"
        };
        h_flex()
            .w_full()
            .pb_2()
            .gap_4()
            .justify_between()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(Headline::new(page.title()).size(HeadlineSize::Small))
                    .child(Label::new(where_kept).size(LabelSize::Small).color(Color::Muted)),
            )
            .child(
                Button::new("settings-open-file", file)
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Subtle)
                    .start_icon(Icon::new(IconName::FileGeneric).size(IconSize::Small))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        window.dispatch_action(file_action.boxed_clone(), cx)
                    })),
            )
    }

    /// What is wrong with settings.json, and where ainet's values come
    /// from when ainetd was not asked.
    fn render_banners(&self, searching: bool, cx: &App) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let banner = |icon: IconName, color: Color, text: String| {
            h_flex()
                .my_1()
                .p_2()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border_variant)
                .child(Icon::new(icon).size(IconSize::Small).color(color))
                .child(Label::new(text).size(LabelSize::Small).color(color))
                .into_any_element()
        };
        if let Some(problem) = problems::get(problems::Source::Settings, cx) {
            out.push(banner(
                IconName::Warning,
                Color::Error,
                format!("settings.json does not read, so the last good settings stay: {problem}"),
            ));
        }
        let ainets = searching || !self.page.is_app();
        if ainets && let Some(report) = &self.report {
            if let Some(error) = report
                .error
                .as_ref()
                .filter(|_| problems::get(problems::Source::Settings, cx).is_none())
            {
                out.push(banner(
                    IconName::Warning,
                    Color::Error,
                    format!("ainet's settings do not read, so the last good ones stay: {error}"),
                ));
            }
            if let Some(notice) = &report.notice {
                out.push(banner(IconName::Info, Color::Warning, notice.clone()));
            }
            if !report.unknown.is_empty() {
                out.push(banner(
                    IconName::Info,
                    Color::Muted,
                    format!(
                        "Ignored, as ainet does not know them: {}",
                        report.unknown.join(", ")
                    ),
                ));
            }
        }
        if ainets && !self.connected(cx) {
            out.push(banner(
                IconName::Info,
                Color::Muted,
                "ainetd is not running: ainet's settings are read from and written to settings.json, and apply when it starts.".into(),
            ));
        }
        out
    }

    /// An ainet category whose page is still to come: where its settings
    /// are until then.
    fn render_placeholder(&self, page: Page) -> impl IntoElement {
        let text = match page.section() {
            Some(section) => format!(
                "This category's page is coming. Until then its settings are in settings.json under \"{section}\" (the schema describes each), and `ainet config list {section}` shows them with their defaults."
            ),
            None => "Paths, versions, the daemon's pid and its log will be shown here. Until then, `ainet config path --all` lists them.".into(),
        };
        div()
            .debug_selector(move || format!("SETTINGS-PLACEHOLDER-{}", page.name()))
            .py_4()
            .child(Label::new(text).color(Color::Muted))
    }

    fn render_field(&self, field: &Field, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = field.key;
        let value = self.value(field, cx);
        let default = self.default(field);
        let part = field.part_of.is_some();
        let modified = !part && !same(&value, &default);
        let less_safe =
            field.owner == Owner::Daemon && ainet_core::settings::less_safe(key, &value) && !value.is_null();
        let default_words = match (&default, field.when_unset) {
            (Value::Null, Some(words)) => words(settings::get(cx)),
            (Value::Null, None) => "unset".into(),
            (d, _) => field.show(d),
        };
        let source = self
            .report
            .as_ref()
            .filter(|_| field.owner == Owner::Daemon)
            .and_then(|r| r.get(key))
            .and_then(|e| match &e.source {
                ainet_core::settings::Source::Env { var } => Some(format!(" · set by ${var}, which wins")),
                _ => None,
            })
            .unwrap_or_default();
        let footnote = if part {
            key.to_string()
        } else {
            format!(
                "Default: {default_words} · applies {} · {key}{source}",
                field.applies().label()
            )
        };
        let wide = matches!(field.kind, Kind::List { .. } | Kind::Map { .. });
        let control = self.render_control(field, &value, window, cx);
        let error = self.errors.get(key).cloned();
        let label = h_flex()
            .gap_1()
            .when(modified, |el| {
                el.child(
                    div()
                        .debug_selector(move || format!("SETTING-MODIFIED-{key}"))
                        .child(Indicator::dot().color(Color::Accent)),
                )
            })
            .child(Label::new(field.label))
            .when(modified, |el| {
                el.child(
                    div()
                        .debug_selector(move || format!("SETTING-RESET-{key}"))
                        .child(
                            IconButton::new(SharedString::from(format!("reset-{key}")), IconName::Undo)
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Reset to Default"))
                                .on_click(cx.listener(move |this, _, _, cx| this.write(key, None, cx))),
                        ),
                )
            })
            .when(less_safe, |el| {
                el.child(
                    div()
                        .debug_selector(move || format!("SETTING-LESS-SAFE-{key}"))
                        .child(
                            Label::new("less safe: agents act without asking you")
                                .size(LabelSize::XSmall)
                                .color(Color::Warning),
                        ),
                )
            });
        // Never narrower than this: in a narrow pane the control goes
        // under it instead.
        let about = v_flex()
            .flex_1()
            .min_w(rems(14.))
            .gap_0p5()
            .child(label)
            .child(
                Label::new(field.description())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Label::new(footnote)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            );
        v_flex()
            .id(SharedString::from(format!("setting-{key}")))
            .debug_selector(move || format!("SETTING-{key}"))
            .w_full()
            .py_3()
            .gap_2()
            .when(part, |el| el.pl_6())
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .map(|el| {
                if wide {
                    el.child(about).child(control)
                } else {
                    el.child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap_x_6()
                            .gap_y_2()
                            .justify_between()
                            .child(about)
                            .child(div().flex_none().child(control)),
                    )
                }
            })
            .when_some(error, |el, error| {
                el.child(
                    div()
                        .debug_selector(move || format!("SETTING-ERROR-{key}"))
                        .child(Label::new(error).size(LabelSize::Small).color(Color::Error)),
                )
            })
            .into_any_element()
    }

    fn render_control(
        &self,
        field: &Field,
        value: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = field.key;
        let selector = move || format!("CONTROL-{key}");
        let id = |what: &str| SharedString::from(format!("{what}-{key}"));
        match &field.kind {
            Kind::Toggle => {
                let on = value.as_bool().unwrap_or_default();
                div()
                    .debug_selector(selector)
                    .child(Switch::new(id("switch"), on.into()).on_click(
                        cx.listener(move |this, _, _, cx| this.write(key, Some(Value::Bool(!on)), cx)),
                    ))
                    .into_any_element()
            }
            Kind::Dropdown(choices) => {
                let current = chosen(choices, value).map(|c| c.label);
                let this = cx.weak_entity();
                let choices = choices.clone();
                let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for choice in choices {
                        let this = this.clone();
                        let picked = Some(choice.label) == current;
                        menu = menu.toggleable_entry(
                            choice.label,
                            picked,
                            IconPosition::Start,
                            None,
                            move |_, cx| {
                                let value = choice.value.clone();
                                let _ = this.update(cx, |this, cx| this.write(key, Some(value), cx));
                            },
                        );
                    }
                    menu
                });
                div()
                    .debug_selector(selector)
                    .child(DropdownMenu::new(id("dropdown"), field.show(value), menu))
                    .into_any_element()
            }
            Kind::Number { unit, .. } => {
                let step = |by: f64, icon: IconName, what: &'static str| {
                    let value = value.clone();
                    div().debug_selector(move || format!("STEP-{what}-{key}")).child(
                        IconButton::new(id(what), icon)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some(field) = this.field(key).cloned() else {
                                    return;
                                };
                                let default = this.default(&field);
                                let to = field.stepped(&value, &default, by);
                                this.write(key, to, cx);
                            })),
                    )
                };
                h_flex()
                    .gap_1()
                    .child(step(-1., IconName::Dash, "DOWN"))
                    // Wide enough for what unset means, when that is words.
                    .child(self.input_box(
                        key,
                        Slot::Value,
                        rems(if field.when_unset.is_some() { 11. } else { 6. }),
                        selector,
                        cx,
                    ))
                    .child(step(1., IconName::Plus, "UP"))
                    .when(!unit.is_empty(), |el| {
                        el.child(Label::new(*unit).size(LabelSize::Small).color(Color::Muted))
                    })
                    .into_any_element()
            }
            Kind::Text { .. } => self
                .input_box(key, Slot::Value, rems(16.), selector, cx)
                .into_any_element(),
            Kind::Path { directory } => {
                let directory = *directory;
                h_flex()
                    .gap_1()
                    .child(self.input_box(key, Slot::Value, rems(20.), selector, cx))
                    .child(
                        div().debug_selector(move || format!("BROWSE-{key}")).child(
                            Button::new(id("browse"), "Browse…")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.browse(key, directory, window, cx)
                                })),
                        ),
                    )
                    .into_any_element()
            }
            Kind::List { .. } => {
                let items: Vec<String> = match value {
                    Value::Array(items) => items
                        .iter()
                        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                        .collect(),
                    _ => Vec::new(),
                };
                v_flex()
                    .w_full()
                    .gap_1()
                    .children(
                        items
                            .into_iter()
                            .enumerate()
                            .map(|(ix, item)| self.entry_row(key, ix, item, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(self.input_box(key, Slot::NewItem, rems(20.), selector, cx))
                            .child(self.add_button(key, Slot::NewItem, cx)),
                    )
                    .into_any_element()
            }
            Kind::Map { .. } => {
                let entries: Vec<String> = match value {
                    Value::Object(map) => map
                        .iter()
                        .map(|(k, v)| match v {
                            Value::String(s) => format!("{k} = {s}"),
                            v => format!("{k} = {v}"),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                v_flex()
                    .w_full()
                    .gap_1()
                    .children(
                        entries
                            .into_iter()
                            .enumerate()
                            .map(|(ix, entry)| self.entry_row(key, ix, entry, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(self.input_box(key, Slot::NewKey, rems(10.), selector, cx))
                            .child(Label::new("=").color(Color::Muted))
                            .child(self.input_box(
                                key,
                                Slot::NewValue,
                                rems(16.),
                                move || format!("CONTROL-VALUE-{key}"),
                                cx,
                            ))
                            .child(self.add_button(key, Slot::NewValue, cx)),
                    )
                    .into_any_element()
            }
            Kind::Theme => {
                let name = match &settings::get(cx).theme {
                    Some(ThemeSelection::Dynamic { light, dark, .. }) => {
                        format!("{} (light {light}, dark {dark})", cx.theme().name)
                    }
                    _ => cx.theme().name.to_string(),
                };
                div()
                    .debug_selector(selector)
                    .child(
                        Button::new(id("theme"), name)
                            .end_icon(
                                Icon::new(IconName::ChevronUpDown)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .tooltip(Tooltip::for_action_title("Select Theme", &theme_selector::Toggle))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(theme_selector::Toggle), cx)
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    /// A text input drawn as a field, `width` wide.
    fn input_box(
        &self,
        key: &'static str,
        slot: Slot,
        width: Rems,
        selector: impl Fn() -> String + 'static,
        cx: &App,
    ) -> Div {
        let input: Option<Entity<TextInput>> = self.inputs.get(&(key, slot)).cloned();
        let colors = cx.theme().colors();
        let error = slot == Slot::Value && self.errors.contains_key(key);
        div()
            .debug_selector(selector)
            .w(width)
            .px_2()
            .py_0p5()
            .rounded_sm()
            .border_1()
            .border_color(if error {
                colors.border_focused
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .children(input)
    }

    fn add_button(&self, key: &'static str, slot: Slot, cx: &mut Context<Self>) -> impl IntoElement {
        div().debug_selector(move || format!("ADD-{key}")).child(
            Button::new(SharedString::from(format!("add-{key}")), "Add")
                .label_size(LabelSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| match slot {
                    Slot::NewItem => this.add_item(key, cx),
                    _ => this.add_entry(key, cx),
                })),
        )
    }

    /// An item of a list or entry of a map, with its remove button.
    fn entry_row(&self, key: &'static str, ix: usize, text: String, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .child(Label::new(text).size(LabelSize::Small).buffer_font(cx))
            .child(
                div().debug_selector(move || format!("REMOVE-{key}-{ix}")).child(
                    IconButton::new(SharedString::from(format!("remove-{key}-{ix}")), IconName::Close)
                        .icon_size(IconSize::XSmall)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text("Remove"))
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_at(key, ix, cx))),
                ),
            )
            .into_any_element()
    }

    /// Asks the system for a file or folder for `key`.
    fn browse(&mut self, key: &'static str, directory: bool, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: !directory,
            directories: directory,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.first()
            {
                let path = Value::String(path.display().to_string());
                let _ = this.update(cx, |this, cx| this.write(key, Some(path), cx));
            }
        })
        .detach();
    }
}
