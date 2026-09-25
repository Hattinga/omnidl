use crate::deps::DepsStatus;
use crate::update;
use serde::Serialize;
use std::path::PathBuf;

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "lowercase")]
pub enum JobState {
    /// Waits for its start time (Unix seconds).
    Scheduled(i64),
    Queued,
    Resolving,
    Downloading,
    Processing,
    /// Container job (playlist/album) whose work happens in child jobs.
    Group,
    Done,
    NoMatch,
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::NoMatch | JobState::Failed(_) | JobState::Cancelled
        )
    }

    /// Worth another try: it did not succeed and nothing is running.
    pub fn can_retry(&self) -> bool {
        matches!(self, JobState::NoMatch | JobState::Failed(_) | JobState::Cancelled)
    }
}

#[derive(Debug)]
pub enum JobUpdate {
    New { parent: Option<JobId>, url: String, title: String, source: &'static str },
    Title(String),
    State(JobState),
    Progress { frac: Option<f32>, speed: Option<f64>, eta: Option<u64> },
    /// Item progress inside one yt-dlp process (non-expanded playlists).
    Item { index: u32, count: u32 },
    Output(PathBuf),
    /// Short remark under the title; an empty string removes it.
    Note(String),
}

#[derive(Debug)]
pub enum Event {
    Job(JobId, JobUpdate),
    Deps(DepsStatus),
    Update(update::Status),
    /// Links arrived from outside the window. `focus`: bring the window forward
    /// (someone started omnidl again, rather than sending from the browser).
    Remote { focus: bool },
}
