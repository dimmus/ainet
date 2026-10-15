//! Following a config file without a file-system watcher: a cheap check of
//! its modification time and size every [`POLL`], off the UI thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{App, Task};

/// How often watched files are checked.
pub const POLL: Duration = Duration::from_millis(750);

/// What tells one version of a file from the next; `None` when it is gone.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The file's contents and stamp, or `None` if it is unchanged since
/// `last`. A file that is gone reads as `Some(None)`.
fn read_if_changed(path: &Path, last: Stamp) -> Option<(Stamp, Option<String>)> {
    let now = stamp(path);
    (now != last).then(|| (now, std::fs::read_to_string(path).ok()))
}

/// Calls `on_change` with the file's new contents (`None` once it is
/// deleted) whenever it changes after now. Stops when the task is dropped.
pub fn watch(
    path: PathBuf,
    cx: &mut App,
    mut on_change: impl FnMut(Option<String>, &mut App) + 'static,
) -> Task<()> {
    let mut last = stamp(&path);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let path = path.clone();
            let changed = cx
                .background_executor()
                .spawn(async move { read_if_changed(&path, last) })
                .await;
            if let Some((now, text)) = changed {
                last = now;
                cx.update(|cx| on_change(text, cx));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_edits_and_deletion() {
        let dir = std::env::temp_dir().join(format!("ainet-gui-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let _ = std::fs::remove_file(&file);

        assert!(read_if_changed(&file, None).is_none(), "still missing");
        std::fs::write(&file, "{}").unwrap();
        let (first, text) = read_if_changed(&file, None).unwrap();
        assert_eq!(text.as_deref(), Some("{}"));
        assert!(read_if_changed(&file, first).is_none(), "unchanged");
        std::fs::write(&file, "{\"ui_font_size\": 14}").unwrap();
        let (second, text) = read_if_changed(&file, first).unwrap();
        assert!(text.unwrap().contains("14"));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read_if_changed(&file, second), Some((None, None)));
        let _ = std::fs::remove_dir(&dir);
    }
}
