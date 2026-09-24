//! The download list as the user sees it: jobs in the order they arrived,
//! children linked to their group, and a one-line status in plain German.
//! Shared by the window, the terminal and the web interface.

use crate::job::{JobId, JobState, JobUpdate};
use crate::{schedule, util};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: JobId,
    pub parent: Option<JobId>,
    /// Indices of the children in [`Jobs::list`].
    #[serde(skip)]
    pub children: Vec<usize>,
    /// The link the job was created from; used to try again.
    pub url: String,
    pub title: String,
    pub source: &'static str,
    pub state: JobState,
    pub frac: Option<f32>,
    /// Bytes per second.
    pub speed: Option<f64>,
    /// Seconds left.
    pub eta: Option<u64>,
    /// Item progress inside a list: (done, total).
    pub item: Option<(u32, u32)>,
    pub note: Option<String>,
    pub output: Option<PathBuf>,
    /// Group shows its children (window only).
    #[serde(skip)]
    pub expanded: bool,
}

/// How a status line should be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Normal,
    Warning,
    Error,
}

impl Job {
    pub fn is_group(&self) -> bool {
        !self.children.is_empty() || self.state == JobState::Group
    }

    /// "YouTube · 45 % · 3.8 MB/s · noch 0:12". Children leave out the source.
    pub fn status(&self, child: bool) -> (String, Tone) {
        let mut parts: Vec<String> = Vec::new();
        if !child {
            parts.push(self.source.to_string());
        }
        let lead = parts.len();
        let mut tone = Tone::Normal;
        match &self.state {
            JobState::Scheduled(at) => parts.push(format!("Startet {}", schedule::describe_unix(*at))),
            JobState::Queued => parts.push(self.note.clone().unwrap_or_else(|| "Wartet".into())),
            JobState::Resolving => parts.push("Suche Quelle …".into()),
            JobState::Processing => parts.push("Wird umgewandelt …".into()),
            JobState::Group => parts.push(match self.item {
                Some((i, n)) => format!("{i} von {n} fertig"),
                None => "Wird vorbereitet …".into(),
            }),
            JobState::Downloading => {
                if let Some((i, n)) = self.item {
                    parts.push(format!("{i} von {n}"));
                }
                if let Some(f) = self.frac {
                    parts.push(format!("{:.0} %", f * 100.0));
                }
                if let Some(s) = self.speed {
                    parts.push(format!("{}/s", util::fmt_bytes(s)));
                }
                if let Some(e) = self.eta {
                    parts.push(format!("noch {}", util::fmt_eta(e)));
                }
                if parts.len() == lead {
                    parts.push("Lädt …".into());
                }
            }
            JobState::Done => parts.push(self.note.clone().unwrap_or_else(|| "Fertig".into())),
            JobState::NoMatch => {
                tone = Tone::Warning;
                parts.push("Kein passender Treffer".into());
            }
            JobState::Cancelled => parts.push("Gestoppt".into()),
            JobState::Failed(e) => {
                tone = Tone::Error;
                parts.push(readable_error(e).to_string());
            }
        }
        (parts.join(" · "), tone)
    }
}

/// First line of a yt-dlp error without the `[extractor] id: ` prefix and the
/// echoed Python exception. The full text stays available elsewhere.
pub fn readable_error(e: &str) -> &str {
    let line = e.lines().next().unwrap_or(e);
    let line = match line.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((_, rest)) => match rest.split_once(": ") {
            Some((id, msg)) if !id.contains(' ') && !msg.is_empty() => msg,
            _ => rest,
        },
        None => line,
    };
    line.split(" (caused by ").next().unwrap_or(line).trim()
}

#[derive(Default)]
pub struct Jobs {
    pub list: Vec<Job>,
    index: HashMap<JobId, usize>,
}

impl Jobs {
    pub fn apply(&mut self, id: JobId, update: JobUpdate) {
        if let JobUpdate::New { parent, url, title, source } = update {
            let idx = self.list.len();
            self.list.push(Job {
                id,
                parent,
                children: Vec::new(),
                url,
                title,
                source,
                state: JobState::Queued,
                frac: None,
                speed: None,
                eta: None,
                item: None,
                note: None,
                output: None,
                expanded: true,
            });
            self.index.insert(id, idx);
            if let Some(p) = parent.and_then(|p| self.index.get(&p).copied()) {
                self.list[p].children.push(idx);
            }
            return;
        }
        let Some(&idx) = self.index.get(&id) else { return };
        let job = &mut self.list[idx];
        match update {
            JobUpdate::New { .. } => {}
            JobUpdate::Title(t) => job.title = t,
            JobUpdate::State(s) => {
                if s.is_finished() {
                    job.speed = None;
                    job.eta = None;
                    if s == JobState::Done {
                        job.frac = Some(1.0);
                    }
                }
                job.state = s;
            }
            JobUpdate::Progress { frac, speed, eta } => {
                job.frac = frac;
                job.speed = speed;
                job.eta = eta;
            }
            JobUpdate::Item { index, count } => job.item = Some((index, count)),
            JobUpdate::Output(p) => job.output = Some(p),
            JobUpdate::Note(n) => job.note = (!n.is_empty()).then_some(n),
        }
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.index.get(&id).map(|&i| &self.list[i])
    }

    pub fn get_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.index.get(&id).map(|&i| &mut self.list[i])
    }

    /// The job itself or anything in its group is still at work.
    pub fn running(&self, idx: usize) -> bool {
        let job = &self.list[idx];
        !job.state.is_finished() || job.children.iter().any(|&c| !self.list[c].state.is_finished())
    }

    /// Drops finished entries; a group stays as long as anything in it runs.
    pub fn clear_finished(&mut self) {
        let keep: Vec<bool> = (0..self.list.len())
            .map(|i| {
                let parent = self.list[i].parent.and_then(|p| self.index.get(&p).copied());
                self.running(i) || parent.is_some_and(|p| self.running(p))
            })
            .collect();
        self.retain(|i, _| keep[i]);
    }

    /// Removes a job together with its children (before trying it again).
    pub fn remove_tree(&mut self, id: JobId) {
        self.retain(|_, j| j.id != id && j.parent != Some(id));
    }

    fn retain(&mut self, keep: impl Fn(usize, &Job) -> bool) {
        let kept: Vec<Job> = std::mem::take(&mut self.list)
            .into_iter()
            .enumerate()
            .filter(|(i, j)| keep(*i, j))
            .map(|(_, j)| j)
            .collect();
        // Rebuild indices and child links.
        self.list = kept;
        self.index = self.list.iter().enumerate().map(|(i, j)| (j.id, i)).collect();
        let links: Vec<Option<usize>> =
            self.list.iter().map(|j| j.parent.and_then(|p| self.index.get(&p).copied())).collect();
        for j in &mut self.list {
            j.children.clear();
        }
        for (child, parent) in links.into_iter().enumerate() {
            if let Some(p) = parent {
                self.list[p].children.push(child);
            }
        }
    }

    /// Top-level jobs newest first, each followed by its children when expanded.
    pub fn visible_rows(&self) -> Vec<(usize, bool)> {
        let mut rows = Vec::new();
        for idx in (0..self.list.len()).rev() {
            let job = &self.list[idx];
            if job.parent.is_some() {
                continue;
            }
            rows.push((idx, false));
            if job.expanded {
                rows.extend(job.children.iter().map(|&c| (c, true)));
            }
        }
        rows
    }

    /// Whether something on screen spins and needs continuous repaints.
    pub fn animating(&self) -> bool {
        self.list.iter().any(|j| match j.state {
            JobState::Resolving | JobState::Processing => true,
            JobState::Downloading | JobState::Group => j.frac.is_none(),
            _ => false,
        })
    }

    pub fn top_level(&self) -> impl Iterator<Item = &Job> {
        self.list.iter().filter(|j| j.parent.is_none())
    }

    /// Children of a group, in the order they arrived.
    pub fn children_of(&self, id: JobId) -> impl Iterator<Item = &Job> {
        let children = self.get(id).map(|j| j.children.clone()).unwrap_or_default();
        children.into_iter().map(move |i| &self.list[i])
    }

    /// Top-level jobs that are neither finished nor scheduled.
    pub fn active(&self) -> usize {
        self.top_level()
            .filter(|j| !j.state.is_finished() && !matches!(j.state, JobState::Scheduled(_)))
            .count()
    }

    pub fn scheduled(&self) -> usize {
        self.top_level().filter(|j| matches!(j.state, JobState::Scheduled(_))).count()
    }

    /// Nothing left to do: every top-level job and its group is finished.
    pub fn all_finished(&self) -> bool {
        (0..self.list.len()).all(|i| !self.running(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(jobs: &mut Jobs, id: JobId, parent: Option<JobId>) {
        let url = format!("https://example.com/{id}");
        jobs.apply(id, JobUpdate::New { parent, url: url.clone(), title: url, source: "Web" });
    }

    fn state(jobs: &mut Jobs, id: JobId, s: JobState) {
        jobs.apply(id, JobUpdate::State(s));
    }

    fn ids(jobs: &Jobs) -> Vec<JobId> {
        jobs.list.iter().map(|j| j.id).collect()
    }

    #[test]
    fn errors_read_like_sentences() {
        assert_eq!(
            readable_error(
                "[generic] nope: Unable to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)"
            ),
            "Unable to download webpage: HTTP Error 404: Not Found"
        );
        assert_eq!(readable_error("[youtube] abc: Video unavailable\nmore"), "Video unavailable");
        assert_eq!(readable_error("Kurzlink nicht auflösbar"), "Kurzlink nicht auflösbar");
        assert_eq!(readable_error("[info] Writing video metadata"), "Writing video metadata");
    }

    #[test]
    fn clearing_keeps_whatever_still_runs() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None); // fertig
        new(&mut jobs, 2, None); // Gruppe, ein Kind läuft noch
        new(&mut jobs, 3, Some(2));
        new(&mut jobs, 4, Some(2));
        new(&mut jobs, 5, None); // läuft
        state(&mut jobs, 1, JobState::Done);
        state(&mut jobs, 2, JobState::Group);
        state(&mut jobs, 3, JobState::Done);
        state(&mut jobs, 4, JobState::Downloading);
        state(&mut jobs, 5, JobState::Queued);

        jobs.clear_finished();
        assert_eq!(ids(&jobs), vec![2, 3, 4, 5], "fertige Kinder laufender Gruppen bleiben");
        assert_eq!(jobs.get(2).unwrap().children.len(), 2, "Verknüpfungen neu aufgebaut");
        assert_eq!(jobs.children_of(2).map(|j| j.id).collect::<Vec<_>>(), vec![3, 4]);

        state(&mut jobs, 4, JobState::Done);
        state(&mut jobs, 2, JobState::Done);
        jobs.clear_finished();
        assert_eq!(ids(&jobs), vec![5]);
        // Updates for the remaining job still land in the right row.
        jobs.apply(5, JobUpdate::Title("neu".into()));
        assert_eq!(jobs.get(5).unwrap().title, "neu");
    }

    #[test]
    fn retry_removes_the_whole_group() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        new(&mut jobs, 2, Some(1));
        new(&mut jobs, 3, None);
        new(&mut jobs, 4, Some(1));
        jobs.remove_tree(1);
        assert_eq!(ids(&jobs), vec![3]);
        assert!(jobs.get(2).is_none());
    }

    #[test]
    fn rows_newest_first_with_children_under_their_group() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        new(&mut jobs, 2, None);
        new(&mut jobs, 3, Some(2));
        new(&mut jobs, 4, Some(2));
        let rows: Vec<(JobId, bool)> = jobs.visible_rows().into_iter().map(|(i, c)| (jobs.list[i].id, c)).collect();
        assert_eq!(rows, vec![(2, false), (3, true), (4, true), (1, false)]);

        jobs.get_mut(2).unwrap().expanded = false;
        assert_eq!(jobs.visible_rows().len(), 2, "zugeklappt: nur die Gruppen");
    }

    #[test]
    fn notes_come_and_go() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        jobs.apply(1, JobUpdate::Note("Wartet auf die Werkzeuge …".into()));
        assert_eq!(jobs.get(1).unwrap().status(false).0, "Web · Wartet auf die Werkzeuge …");
        jobs.apply(1, JobUpdate::Note(String::new()));
        assert_eq!(jobs.get(1).unwrap().status(false).0, "Web · Wartet");
    }

    #[test]
    fn scheduled_jobs_show_their_start() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        let at = schedule::to_unix(schedule::next_at(schedule::local_now(), 23, 55));
        state(&mut jobs, 1, JobState::Scheduled(at));
        let (line, tone) = jobs.get(1).unwrap().status(false);
        assert!(line.starts_with("Web · Startet "), "{line}");
        assert!(line.ends_with("um 23:55"), "{line}");
        assert_eq!(tone, Tone::Normal);
        assert!(!jobs.animating(), "geplante Jobs brauchen kein Neuzeichnen");
        assert_eq!((jobs.active(), jobs.scheduled()), (0, 1));
        assert!(!jobs.all_finished());
        assert!(!JobState::Scheduled(at).is_finished());
        assert!(!JobState::Scheduled(at).can_retry());
    }

    #[test]
    fn progress_and_outcomes_read_naturally() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        state(&mut jobs, 1, JobState::Downloading);
        jobs.apply(1, JobUpdate::Progress { frac: Some(0.45), speed: Some(3.8 * 1024.0 * 1024.0), eta: Some(12) });
        assert_eq!(jobs.get(1).unwrap().status(false).0, "Web · 45 % · 3.8 MB/s · noch 0:12");
        assert_eq!(jobs.get(1).unwrap().status(true).0, "45 % · 3.8 MB/s · noch 0:12", "Kind ohne Quelle");

        state(&mut jobs, 1, JobState::Failed("[youtube] x: Video unavailable".into()));
        assert_eq!(jobs.get(1).unwrap().status(false), ("Web · Video unavailable".into(), Tone::Error));
        assert!(jobs.all_finished());
        state(&mut jobs, 1, JobState::NoMatch);
        assert_eq!(jobs.get(1).unwrap().status(false).1, Tone::Warning);
    }

    #[test]
    fn serializes_for_the_web_interface() {
        let mut jobs = Jobs::default();
        new(&mut jobs, 1, None);
        state(&mut jobs, 1, JobState::Failed("kaputt".into()));
        let v = serde_json::to_value(jobs.get(1).unwrap()).unwrap();
        assert_eq!(v["state"], serde_json::json!({ "kind": "failed", "detail": "kaputt" }));
        assert_eq!(v["source"], "Web");
        assert!(v.get("children").is_none(), "interne Indizes bleiben intern");
        state(&mut jobs, 1, JobState::Done);
        let v = serde_json::to_value(jobs.get(1).unwrap()).unwrap();
        assert_eq!(v["state"], serde_json::json!({ "kind": "done" }));
    }
}
