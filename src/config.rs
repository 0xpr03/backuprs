use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;
use miette::{bail, Context, IntoDiagnostic, Result};

use crate::job::Job;

#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Global {
    /// Repository backend and defaults
    #[serde(flatten)]
    pub backend: RepositoryData,
    /// Path to restic binary
    pub restic_binary: PathBuf,
    /// Default interval to use for backup jobs
    #[serde_as(as = "DurationSeconds<u64>")]
    pub default_interval: Duration,
    /// All backup jobs
    pub jobs: Vec<Job>,
}

impl Global {
    pub fn repo_url(&self, job: &Job) -> Result<String> {
        match &self.backend {
            RepositoryData::Restic { restic_url, server_pubkey_file } => {
                let mut url = String::from("rest:");
                if server_pubkey_file.is_some() {
                    url.push_str("https://");
                } else {
                    url.push_str("http://");
                }
                url.push_str(&job.user);
                url.push_str(":");
                url.push_str(&job.password);
                url.push_str("@");
                url.push_str(&restic_url);
                url.push_str("/");
                url.push_str(&job.repository);
                Ok(url)
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub enum RepositoryData {
    /// Restic rest-server backend
    Restic {
        /// Repostiroy URL of the restic server.
        /// Does not contain the repo or user/password.
        restic_url: String,
        /// Pubkey for the server when HTTPS is used.
        server_pubkey_file: Option<PathBuf>,
    },
}

