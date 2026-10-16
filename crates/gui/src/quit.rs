//! Quitting, and closing windows: closing the last window quits, so with
//! `confirm_quit` set every way out asks first, whether it is `ctrl-q`,
//! `workspace::CloseWindow`, the title bar's close button or the window
//! manager.

use gpui::{App, PromptLevel, Window};

use crate::actions::Quit;
use crate::settings;

/// Quits, asking first in the active window if `confirm_quit` is set.
pub fn quit(_: &Quit, cx: &mut App) {
    let window = cx.active_window().or_else(|| cx.windows().into_iter().next());
    let Some(window) = window.filter(|_| settings::get(cx).confirm_quit) else {
        cx.quit();
        return;
    };
    if window.update(cx, |_, window, cx| ask(window, cx)).is_err() {
        cx.quit();
    }
}

/// Closes `window`, asking first if that would quit.
pub fn close_window(window: &mut Window, cx: &mut App) {
    if must_ask_on_close(cx) {
        ask(window, cx);
    } else {
        window.remove_window();
    }
}

/// Whether closing a window now must ask: it is the last one, and
/// `confirm_quit` is set.
pub fn must_ask_on_close(cx: &App) -> bool {
    settings::get(cx).confirm_quit && cx.windows().len() <= 1
}

/// Asks in `window` whether to quit, and quits if so.
pub fn ask(window: &mut Window, cx: &mut App) {
    let answer = window.prompt(PromptLevel::Info, "Quit ainet?", None, &["Quit", "Cancel"], cx);
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            cx.update(|cx| cx.quit());
        }
    })
    .detach();
}
