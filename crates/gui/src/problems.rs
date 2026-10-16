//! What is wrong with the user's config: a settings or keymap file that
//! does not parse, an unknown theme, a bundled file that failed to load.
//! Kept as a GPUI global until fixed, shown in the status bar, and logged
//! to stderr once when it appears.

use gpui::{App, Global};

/// Where a problem comes from; each has at most one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Startup,
    Settings,
    Theme,
    /// The user's theme files.
    Themes,
    Keymap,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Startup => "ainet-gui",
            Source::Settings => "settings.json",
            Source::Theme => "theme",
            Source::Themes => "themes",
            Source::Keymap => "keymap.json",
        }
    }
}

#[derive(Default)]
pub struct Problems(Vec<(Source, String)>);

impl Global for Problems {}

/// Records `problem` for `source`, or clears it with `None`.
pub fn set(source: Source, problem: Option<String>, cx: &mut App) {
    let problems = &mut cx.default_global::<Problems>().0;
    let old = problems.iter().position(|(s, _)| *s == source);
    let unchanged = match (&old, &problem) {
        (Some(i), Some(p)) => problems[*i].1 == *p,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }
    if let Some(i) = old {
        problems.remove(i);
    }
    if let Some(problem) = problem {
        eprintln!("ainet-gui: {}: {problem}", source.label());
        problems.push((source, problem));
        problems.sort_by_key(|(s, _)| *s);
    }
    cx.refresh_windows();
}

/// Every current problem as one line each, for the status bar.
pub fn lines(cx: &App) -> Vec<String> {
    cx.try_global::<Problems>()
        .map(|p| {
            p.0.iter()
                .map(|(s, text)| format!("{}: {text}", s.label()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn kept_until_cleared(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert!(lines(cx).is_empty());
            set(Source::Keymap, Some("bad key".into()), cx);
            set(Source::Settings, Some("line 3".into()), cx);
            assert_eq!(lines(cx), ["settings.json: line 3", "keymap.json: bad key"]);
            set(Source::Settings, Some("line 4".into()), cx);
            set(Source::Keymap, None, cx);
            assert_eq!(lines(cx), ["settings.json: line 4"]);
        });
    }
}
