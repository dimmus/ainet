//! The theme selector (`theme_selector::Toggle`), as Zed's: every theme in
//! the registry (bundled, Omarchy, your own), dark ones first, the one in
//! use ticked and selected. Moving the selection previews a theme on the
//! whole window; escape (or a click outside) puts the one in use back, and
//! picking one saves it as `"theme"` in settings.json, leaving the rest of
//! the file as it is.

use std::sync::Arc;

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{DismissEvent, Entity, Task};
use theme::{GlobalTheme, Theme, ThemeRegistry};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};

use crate::fleet::FleetModel;
use crate::picker::{Picker, PickerDelegate, match_strings};
use crate::settings;

pub struct ThemeSelector {
    fleet: Entity<FleetModel>,
    /// The themes' names, dark first, then by name.
    candidates: Arc<Vec<StringMatchCandidate>>,
    matches: Vec<StringMatch>,
    selected: usize,
    /// The theme in use when the selector opened, to put back.
    original: Arc<Theme>,
    /// The theme on show.
    shown: Option<SharedString>,
    /// A theme was picked: closing keeps it.
    picked: bool,
}

impl ThemeSelector {
    pub fn new(fleet: Entity<FleetModel>, cx: &App) -> Self {
        let mut themes = ThemeRegistry::global(cx).list();
        themes.sort_by(|a, b| {
            a.appearance
                .is_light()
                .cmp(&b.appearance.is_light())
                .then(a.name.cmp(&b.name))
        });
        let candidates = themes
            .iter()
            .enumerate()
            .map(|(ix, t)| StringMatchCandidate::new(ix, &t.name))
            .collect();
        ThemeSelector {
            fleet,
            candidates: Arc::new(candidates),
            matches: Vec::new(),
            selected: 0,
            original: cx.theme().clone(),
            shown: None,
            picked: false,
        }
    }

    /// Shows the selected theme on the window.
    fn preview(&mut self, cx: &mut App) {
        let Some(m) = self.matches.get(self.selected) else {
            return;
        };
        if self.shown.as_deref() == Some(m.string.as_str()) {
            return;
        }
        if let Ok(theme) = ThemeRegistry::global(cx).get(&m.string) {
            self.shown = Some(m.string.clone().into());
            show(theme, cx);
        }
    }
}

/// Makes `theme` the one the window is drawn in, for now.
fn show(theme: Arc<Theme>, cx: &mut App) {
    if !Arc::ptr_eq(cx.theme(), &theme) {
        GlobalTheme::update_theme(cx, theme);
        cx.refresh_windows();
    }
}

impl PickerDelegate for ThemeSelector {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected = ix;
        self.preview(cx);
    }

    fn placeholder_text(&self, _: &App) -> SharedString {
        "Select Theme…".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let found = match_strings(self.candidates.clone(), query, cx);
        cx.spawn(async move |picker, cx| {
            let matches = found.await;
            let _ = picker.update(cx, |picker, cx| {
                let this = &mut picker.delegate;
                // The theme on show stays selected while it matches; at
                // first, that is the one in use.
                let keep = this.shown.clone().unwrap_or_else(|| this.original.name.clone());
                this.selected = matches
                    .iter()
                    .position(|m| m.string == keep.as_ref())
                    .unwrap_or(0);
                this.matches = matches;
                this.preview(cx);
            });
        })
    }

    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(m) = self.matches.get(self.selected) else {
            return;
        };
        self.picked = true;
        if let Err(why) = settings::save_theme(&m.string, cx) {
            self.fleet.update(cx, |fleet, cx| fleet.show(why, true, cx));
        }
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        if !self.picked {
            show(self.original.clone(), cx);
        }
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<ListItem> {
        let m = self.matches.get(ix)?;
        let in_use = m.string == self.original.name.as_ref();
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(HighlightedLabel::new(m.string.clone(), m.positions.clone()))
                .when(in_use, |item| {
                    item.end_slot(Icon::new(IconName::Check).color(Color::Muted))
                }),
        )
    }

    fn render_footer(&self, _: &mut Window, cx: &mut Context<Picker<Self>>) -> Option<AnyElement> {
        let dir = crate::themes::user_dir();
        Some(
            h_flex()
                .w_full()
                .p_2()
                .gap_2()
                .justify_between()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Label::new(format!("Your own: Zed theme files in {}", dir.display()))
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate(),
                )
                .child(
                    Button::new("theme-folder", "Open Folder").on_click(move |_, _, cx| {
                        let _ = std::fs::create_dir_all(&dir);
                        cx.open_with_system(&dir);
                    }),
                )
                .into_any_element(),
        )
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use gpui::{Modifiers, TestAppContext, point, px};

    use super::*;
    use crate::actions::theme_selector::Toggle;
    use crate::tests::{demo_workspace, modal};

    #[gpui::test]
    async fn previews_as_it_moves_restores_on_escape_and_saves_the_pick(cx: &mut TestAppContext) {
        let (w, cx) = demo_workspace(cx);
        let dir = std::env::temp_dir().join(format!("ainet-gui-theme-selector-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let text = "// Mine.\n{\n  \"ui_font_size\": 15, // big\n  \"theme\": \"One Dark\",\n}\n";
        std::fs::write(&file, text).unwrap();
        cx.update(|_, cx| {
            settings::save_to(file.clone(), cx);
            settings::reload(text, cx);
        });
        let theme = |cx: &mut gpui::VisualTestContext| cx.update(|_, cx| cx.theme().name.to_string());
        assert_eq!(theme(cx), "One Dark");

        cx.dispatch_action(Toggle);
        let picker = modal::<Picker<ThemeSelector>>(&w, cx).expect("the selector");
        let selected = picker.read_with(cx, |p, _| p.delegate.matches[p.delegate.selected].string.clone());
        assert_eq!(selected, "One Dark", "the one in use is selected");
        cx.simulate_keystrokes("down");
        let previewed = theme(cx);
        assert_ne!(previewed, "One Dark", "moving previews");
        cx.simulate_keystrokes("escape");
        assert!(modal::<Picker<ThemeSelector>>(&w, cx).is_none());
        assert_eq!(theme(cx), "One Dark", "escape puts it back");

        // A click outside gives up too.
        cx.dispatch_action(Toggle);
        cx.simulate_keystrokes("down");
        cx.simulate_click(point(px(4.), px(600.)), Modifiers::none());
        assert_eq!(theme(cx), "One Dark");

        cx.dispatch_action(Toggle);
        cx.simulate_input("ayu light");
        assert_eq!(theme(cx), "Ayu Light", "the match on show");
        cx.simulate_keystrokes("enter");
        assert!(modal::<Picker<ThemeSelector>>(&w, cx).is_none());
        assert_eq!(theme(cx), "Ayu Light");
        let saved = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            saved,
            text.replace("One Dark", "Ayu Light"),
            "only the theme changed"
        );
        cx.update(|_, cx| {
            assert_eq!(
                settings::get(cx).theme,
                Some(settings::ThemeSelection::Static("Ayu Light".into()))
            );
            assert_eq!(settings::get(cx).ui_font_size, 15.);
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
