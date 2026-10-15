//! Following a config file, or a directory of them, without a file-system
//! watcher: a cheap check of modification times, sizes and contents every
//! [`POLL`], off the UI thread.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{App, Task};

/// How often watched files are checked.
pub const POLL: Duration = Duration::from_millis(750);

/// What tells one version of a file from the next: its modification
/// time, size and a hash of its contents (a quick re-save of the same
/// size can keep the time on a coarse file system); `None` when it is
/// gone. Config files are small, so reading them each time costs little.
type Stamp = Option<(SystemTime, u64, u64)>;

fn stamp_of(path: &Path, bytes: &[u8]) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some((meta.modified().ok()?, meta.len(), hasher.finish()))
}

fn stamp(path: &Path) -> Stamp {
    stamp_of(path, &std::fs::read(path).ok()?)
}

/// The file's contents and stamp, or `None` if it is unchanged since
/// `last`. A file that is gone reads as `Some(None)`.
fn read_if_changed(path: &Path, last: Stamp) -> Option<(Stamp, Option<String>)> {
    let bytes = std::fs::read(path).ok();
    let now = bytes.as_deref().and_then(|b| stamp_of(path, b));
    (now != last).then(|| (now, bytes.and_then(|b| String::from_utf8(b).ok())))
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

/// What tells one version of a directory's `.json` files from the next.
type DirStamp = Vec<(PathBuf, Stamp)>;

fn dir_stamp(dir: &Path) -> DirStamp {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| (stamp(&p), p))
        .map(|(s, p)| (p, s))
        .collect()
}

/// Calls `on_change` whenever a `.json` file in `dir` is added, changed or
/// removed after now (the directory may come and go too). Stops when the
/// task is dropped.
pub fn watch_dir(dir: PathBuf, cx: &mut App, mut on_change: impl FnMut(&mut App) + 'static) -> Task<()> {
    let mut last = dir_stamp(&dir);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            let dir = dir.clone();
            let now = cx
                .background_executor()
                .spawn(async move { dir_stamp(&dir) })
                .await;
            if now != last {
                last = now;
                cx.update(|cx| on_change(cx));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_changes_with_its_json_files() {
        let dir = std::env::temp_dir().join(format!("ainet-gui-watch-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gone = dir_stamp(&dir);
        assert!(gone.is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        assert_eq!(dir_stamp(&dir), gone, "only .json files count");
        std::fs::write(dir.join("a.json"), "{}").unwrap();
        let one = dir_stamp(&dir);
        assert_eq!(one.len(), 1);
        std::fs::write(dir.join("a.json"), "{ \"name\": 1 }").unwrap();
        assert_ne!(dir_stamp(&dir), one, "an edit");
        std::fs::remove_file(dir.join("a.json")).unwrap();
        assert!(dir_stamp(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A re-save of the same size within the file system's time
    /// resolution: the contents tell.
    #[test]
    fn notices_a_same_size_edit_at_the_same_time() {
        let dir = std::env::temp_dir().join(format!("ainet-gui-watch-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("theme.json");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let save = |text: &str| {
            std::fs::write(&file, text).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        save("{ \"bg\": \"#101418\" }");
        let (first, _) = read_if_changed(&file, None).unwrap();
        save("{ \"bg\": \"#203040\" }");
        let (_, text) = read_if_changed(&file, first).expect("seen");
        assert!(text.unwrap().contains("#203040"));
        let dir_first = dir_stamp(&dir);
        save("{ \"bg\": \"#304050\" }");
        assert_ne!(dir_stamp(&dir), dir_first);
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
