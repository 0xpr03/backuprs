use serde::Deserialize;
use std::fmt::Display;
use time::OffsetDateTime;

pub type Snapshots = Vec<Snapshot>;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(with = "time::serde::iso8601")]
    pub time: OffsetDateTime,
    pub paths: Vec<String>,
    pub hostname: String,
    pub username: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "message_type")]
pub enum BackupMessage {
    #[serde(rename = "verbose_status")]
    VerboseStatus(BackupVerboseStatus),
    #[serde(rename = "status")]
    Status(BackupStatus),
    #[serde(rename = "summary")]
    Summary(BackupSummary),
}

/// For some reason restic outputs 2 different kinds of normal status.
/// One for intermediate steps, and one on finish.
///
/// The difference is that the finish status contains an action : scan_finished thingy
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BackupStatus {
    Finish(BackupStatusFinish),
    Intermediate(BackupStatusIntermediate),
}

#[derive(Debug, Deserialize)]
pub struct BackupStatusFinish {
    pub action: String,
    pub duration: f64,
    pub data_size: usize,
    pub data_size_in_repo: usize,
    pub metadata_size: usize,
    pub metadata_size_in_repo: usize,
    pub total_files: usize,
}

#[derive(Debug, Deserialize)]
pub struct BackupStatusIntermediate {
    pub percent_done: f64,
    #[serde(default)]
    pub total_files: usize,
    #[serde(default)]
    pub files_done: usize,
    #[serde(default)]
    pub total_bytes: usize,
    #[serde(default)]
    pub bytes_done: usize,
}

#[derive(Debug, Deserialize)]
pub struct BackupVerboseStatus {
    pub action: String,
    pub item: String,
    pub duration: f64,
    pub data_size: usize,
    pub data_size_in_repo: usize,
    pub metadata_size: usize,
    pub metadata_size_in_repo: usize,
    pub total_files: usize,
}

/// Returned from restic after a successfull backup
#[derive(Debug, Deserialize)]
pub struct BackupSummary {
    // pub message_type":"summary
    pub files_new: usize,
    pub files_changed: usize,
    pub files_unmodified: usize,
    pub dirs_new: usize,
    pub dirs_changed: usize,
    pub dirs_unmodified: usize,
    pub data_blobs: usize,
    pub tree_blobs: usize,
    pub data_added: usize,
    pub total_files_processed: usize,
    pub total_bytes_processed: usize,
    pub total_duration: f32,
    pub snapshot_id: String,
}

impl Display for BackupSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (added_unit, added) = format_size(self.data_added);
        f.write_fmt(format_args!("took {}s, {added} {added_unit} added, {} new files, {} changed files, {} unchanged files",
        self.total_duration,self.files_new,self.files_changed,self.files_unmodified))
    }
}


pub const fn format_size(bytes: usize) -> (&'static str, usize) {
    if bytes > 2 << 40 {
        ("TiB", bytes / (2 << 40))
    } else if bytes > 2 << 30 {
        ("GiB", bytes / (2 << 30))
    } else if bytes > 2 << 20 {
        ("MiB", bytes / (2 << 20))
    } else if bytes > 2 << 10 {
        ("KiB", bytes / (2 << 10))
    } else {
        ("B", bytes)
    }
}