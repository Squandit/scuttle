//! Remembers which results get used, so the things you open most and
//! most recently float to the top.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How many entries are kept on disk.
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    count: u32,
    /// Seconds since the Unix epoch.
    last_used: u64,
}

/// Usage counts for launcher results, keyed by [`crate::Item::key`].
#[derive(Debug, Default)]
pub struct History {
    path: Option<PathBuf>,
    entries: HashMap<String, Entry>,
}

impl History {
    /// A history that is never written to disk.
    pub fn in_memory() -> History {
        History::default()
    }

    /// Loads history from `path`. A missing or unreadable file gives an
    /// empty history that will be saved to the same place.
    pub fn load(path: &Path) -> History {
        let mut history = History { path: Some(path.to_path_buf()), entries: HashMap::new() };
        let Ok(text) = fs::read_to_string(path) else { return history };
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(count), Some(last), Some(key)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(count), Ok(last_used)) = (count.parse(), last.parse()) else { continue };
            if !key.is_empty() {
                history.entries.insert(key.to_string(), Entry { count, last_used });
            }
        }
        history
    }

    /// Notes that `key` was just used.
    pub fn record(&mut self, key: &str, now: u64) {
        let entry = self.entries.entry(key.to_string()).or_insert(Entry { count: 0, last_used: now });
        entry.count = entry.count.saturating_add(1);
        entry.last_used = now;
    }

    /// A ranking bonus for `key`: frequent and recent use scores higher.
    /// Never-used keys score 0.
    pub fn boost(&self, key: &str, now: u64) -> u32 {
        let Some(entry) = self.entries.get(key) else { return 0 };
        let age = now.saturating_sub(entry.last_used);
        const HOUR: u64 = 3600;
        const DAY: u64 = 24 * HOUR;
        let recency = match age {
            a if a < HOUR => 8,
            a if a < DAY => 6,
            a if a < 7 * DAY => 4,
            a if a < 30 * DAY => 2,
            _ => 1,
        };
        entry.count.min(20) * recency
    }

    /// Writes the history to disk, keeping only the most valuable
    /// entries. Does nothing for an in-memory history.
    pub fn save(&self, now: u64) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut rows: Vec<(&String, &Entry)> = self.entries.iter().collect();
        rows.sort_by(|a, b| {
            self.boost(b.0, now).cmp(&self.boost(a.0, now)).then_with(|| a.0.cmp(b.0))
        });
        rows.truncate(MAX_ENTRIES);
        let mut text = String::new();
        for (key, entry) in rows {
            // Keys never contain tabs or newlines, but make sure a
            // strange one cannot corrupt the file.
            let key = key.replace(['\t', '\n'], " ");
            text.push_str(&format!("{}\t{}\t{}\n", entry.count, entry.last_used, key));
        }
        // Write to a temporary file first so a crash cannot leave a
        // half-written history behind.
        let temp = path.with_extension("tmp");
        fs::write(&temp, text)?;
        fs::rename(&temp, path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn unused_keys_get_no_boost() {
        assert_eq!(History::in_memory().boost("app:firefox", NOW), 0);
    }

    #[test]
    fn more_use_means_more_boost() {
        let mut h = History::in_memory();
        h.record("a", NOW);
        h.record("b", NOW);
        h.record("b", NOW);
        assert!(h.boost("b", NOW) > h.boost("a", NOW));
    }

    #[test]
    fn recent_use_beats_old_use() {
        let mut h = History::in_memory();
        h.record("recent", NOW - 60);
        h.record("old", NOW - 90 * 24 * 3600);
        assert!(h.boost("recent", NOW) > h.boost("old", NOW));
    }

    #[test]
    fn boost_is_capped_so_one_app_cannot_dominate_forever() {
        let mut h = History::in_memory();
        for _ in 0..1000 {
            h.record("a", NOW);
        }
        assert_eq!(h.boost("a", NOW), 20 * 8);
    }

    #[test]
    fn survives_a_round_trip_to_disk() {
        let dir = std::env::temp_dir().join(format!("scuttle-history-{}", std::process::id()));
        let path = dir.join("nested").join("history");
        let mut h = History::load(&path);
        assert!(h.is_empty());
        h.record("app:firefox", NOW);
        h.record("app:firefox", NOW);
        h.record("file:/home/me/a b.txt", NOW - 10);
        h.save(NOW).unwrap();

        let loaded = History::load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.boost("app:firefox", NOW), h.boost("app:firefox", NOW));
        assert_eq!(loaded.boost("file:/home/me/a b.txt", NOW), 8);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_lines_are_skipped() {
        let dir = std::env::temp_dir().join(format!("scuttle-history-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history");
        fs::write(&path, "garbage\n3\t100\tgood\nx\ty\tbad\n\n2\t100\t\n").unwrap();
        let h = History::load(&path);
        assert_eq!(h.len(), 1);
        assert!(h.boost("good", 200) > 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_memory_history_never_touches_disk() {
        let mut h = History::in_memory();
        h.record("a", NOW);
        h.save(NOW).unwrap();
    }
}
