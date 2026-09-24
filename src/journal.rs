//! Downloads that have not finished yet, written to disk on every change so
//! closing the app (or a crash, or an update) does not lose them.

use crate::config::Config;
use crate::job::JobId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    /// Settings at the time the link was added.
    pub cfg: Config,
    /// Unix seconds; `None` starts right away.
    #[serde(default)]
    pub start_at: Option<i64>,
    /// Items of a playlist or album that are already done (their keys).
    #[serde(default)]
    pub done: Vec<String>,
}

impl Entry {
    pub fn new(url: String, cfg: Config, start_at: Option<i64>) -> Self {
        Self { url, cfg, start_at, done: Vec::new() }
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    pending: Vec<Entry>,
}

pub struct Journal {
    /// `None` keeps everything in memory (tests).
    path: Option<PathBuf>,
    entries: Mutex<Vec<(JobId, Entry)>>,
}

impl Journal {
    /// Keeps nothing on disk (one-off downloads from the terminal, tests).
    pub fn in_memory() -> Self {
        Self { path: None, entries: Mutex::new(Vec::new()) }
    }

    /// Opens the journal and hands out what was left over from last time.
    /// The file keeps those entries until they are added again under new ids.
    pub fn open(path: PathBuf) -> (Self, Vec<Entry>) {
        let pending = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<File>(&s).ok())
            .map(|f| f.pending)
            .unwrap_or_default();
        (Self { path: Some(path), entries: Mutex::new(Vec::new()) }, pending)
    }

    pub fn insert(&self, id: JobId, entry: Entry) {
        self.change(|list| list.push((id, entry)));
    }

    /// The scheduled time has come; a restart should start it right away.
    pub fn started(&self, id: JobId) {
        self.change(|list| {
            if let Some((_, e)) = list.iter_mut().find(|(i, _)| *i == id) {
                e.start_at = None;
            }
        });
    }

    pub fn mark_done(&self, id: JobId, key: &str) {
        self.change(|list| {
            if let Some((_, e)) = list.iter_mut().find(|(i, _)| *i == id) {
                if !e.done.iter().any(|k| k == key) {
                    e.done.push(key.to_string());
                }
            }
        });
    }

    /// Finished, failed or cancelled: nothing left to resume.
    pub fn remove(&self, id: JobId) {
        self.change(|list| list.retain(|(i, _)| *i != id));
    }

    #[cfg(test)]
    pub fn get(&self, id: JobId) -> Option<Entry> {
        self.entries.lock().unwrap().iter().find(|(i, _)| *i == id).map(|(_, e)| e.clone())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    fn change(&self, f: impl FnOnce(&mut Vec<(JobId, Entry)>)) {
        let mut list = self.entries.lock().unwrap();
        let before = list.clone();
        f(&mut list);
        if *list != before {
            // Written under the lock so saves cannot overtake each other.
            self.save(&list);
        }
    }

    fn save(&self, list: &[(JobId, Entry)]) {
        let Some(path) = &self.path else { return };
        let file = File { version: 1, pending: list.iter().map(|(_, e)| e.clone()).collect() };
        let Ok(json) = serde_json::to_string_pretty(&file) else { return };
        // Write, then swap: a crash mid-write leaves the previous file intact.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("omnidl-tests").join("journal");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    fn entry(url: &str) -> Entry {
        Entry::new(url.into(), Config::default(), None)
    }

    #[test]
    fn unfinished_downloads_survive_a_restart() {
        let p = path("restart.json");
        let (j, left) = Journal::open(p.clone());
        assert!(left.is_empty());
        j.insert(1, entry("https://a"));
        j.insert(2, Entry::new("https://b".into(), Config::default(), Some(1_900_000_000)));
        j.insert(3, entry("https://c"));
        j.mark_done(3, "https://c/1");
        j.mark_done(3, "https://c/1");
        j.remove(1);
        drop(j);

        let (_, left) = Journal::open(p.clone());
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].url, "https://b");
        assert_eq!(left[0].start_at, Some(1_900_000_000));
        assert_eq!(left[1].done, vec!["https://c/1"], "doppelt gemeldet, einmal gespeichert");
        assert!(!p.with_extension("json.tmp").exists(), "keine Zwischendatei bleibt liegen");
    }

    #[test]
    fn started_schedules_resume_immediately() {
        let p = path("started.json");
        let (j, _) = Journal::open(p.clone());
        j.insert(5, Entry::new("https://x".into(), Config::default(), Some(1_900_000_000)));
        j.started(5);
        assert_eq!(j.get(5).unwrap().start_at, None);
        let (_, left) = Journal::open(p);
        assert_eq!(left[0].start_at, None);
    }

    #[test]
    fn leftovers_stay_on_disk_until_readded() {
        let p = path("readd.json");
        let (j, _) = Journal::open(p.clone());
        j.insert(1, entry("https://a"));
        drop(j);
        // Zweiter Start liest, stürzt aber ab, bevor er etwas neu einreiht.
        let (_, left) = Journal::open(p.clone());
        assert_eq!(left.len(), 1);
        let (j, left) = Journal::open(p.clone());
        assert_eq!(left.len(), 1, "nichts geht beim bloßen Lesen verloren");
        j.insert(9, left[0].clone());
        j.remove(9);
        assert!(Journal::open(p).1.is_empty());
    }

    #[test]
    fn a_damaged_file_is_ignored() {
        let p = path("broken.json");
        std::fs::write(&p, "{ kaputt").unwrap();
        let (j, left) = Journal::open(p);
        assert!(left.is_empty());
        j.insert(1, entry("https://a"));
        assert_eq!(j.len(), 1);
    }

    #[test]
    fn memory_journal_writes_nothing() {
        let j = Journal::in_memory();
        j.insert(1, entry("https://a"));
        j.mark_done(1, "k");
        assert_eq!(j.get(1).unwrap().done, vec!["k"]);
        j.remove(1);
        assert_eq!(j.len(), 0);
    }
}
