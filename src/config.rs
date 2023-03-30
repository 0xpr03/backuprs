use std::fmt::Display;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

use miette::{bail, Result};
use miette::{Context, IntoDiagnostic};
use serde::de;
use serde::Deserialize;
use serde::Deserializer;
use time::format_description;

use crate::job::Job;
use crate::job::JobMap;

#[derive(Deserialize)]
pub struct Conf {
    pub global: Global,
    /// All backup jobs
    pub job: Vec<JobData>,
}

impl Conf {
    pub fn split(self) -> Result<(Defaults, JobMap)> {
        self.global.check()?;
        let defaults = Rc::new(self.global);
        let jobs: Result<JobMap> = self
            .job
            .into_iter()
            .map(|v| {
                let name = v.name.clone();
                let job = Job::new(v, defaults.clone())?;
                Ok((name, job))
            })
            .collect();

        Ok((defaults, jobs?))
    }
}

pub type Defaults = Rc<Global>;

#[derive(Debug, Deserialize)]
pub struct Global {
    /// Repository backend and defaults
    #[serde(flatten)]
    pub backend: RepositoryData,
    /// Path to restic binary
    pub restic_binary: PathBuf,
    /// Verbose output, passed via CLI params
    #[serde(default)]
    pub verbose: bool,
    /// Default interval to use for backup jobs
    pub default_interval: u64,
    /// Period of time to perform backup jobs
    pub period: Option<BackupTimeRange>,
}

#[derive(Debug, Deserialize)]
pub struct BackupTimeRange {
    /// Backup time start
    #[serde(deserialize_with = "deserialize_time")]
    pub backup_start_time: time::Time,
    /// Backup time end
    #[serde(deserialize_with = "deserialize_time")]
    pub backup_end_time: time::Time,
}

/// Deserialize a type `S` by deserializing a string, then using the `FromStr`
/// impl of `S` to create the result. The generic type `S` is not required to
/// implement `Deserialize`.
fn deserialize_time<'de, D>(deserializer: D) -> Result<time::Time, D::Error>
where
    D: Deserializer<'de>,
{
    let string: String = Deserialize::deserialize(deserializer)?;
    let time_fmt = format_description::parse("[hour]:[minute]").map_err(de::Error::custom)?;
    time::Time::parse(&string, &time_fmt).map_err(de::Error::custom)
}

impl Global {
    /// Verify basic validity
    pub fn check(&self) -> Result<()> {
        if !self.restic_binary.exists() {
            bail!("Path for config value 'restic_binary' not accessible or doesn't exist!");
        }
        if !self.restic_binary.is_file() {
            bail!("Path for config value 'restic_binary' is not a file!");
        }
        if let Some(period) = &self.period {
            if period.backup_start_time == period.backup_end_time {
                bail!("Backup period start and end time can't be the same!");
            }
        }
        match &self.backend {
            RepositoryData::Rest {
                rest_url: _,
                server_pubkey_file,
            } => {
                if let Some(pubkey_file) = server_pubkey_file {
                    if !pubkey_file.exists() {
                        bail!("Rest 'server_pubkey_file' specified, but file does not exist?");
                    }
                    std::fs::File::open(&pubkey_file)
                        .into_diagnostic()
                        .wrap_err("Rest 'server_pubkey_file' specified, but can't read file?")?;
                }
            }
        }
        Ok(())
    }
    pub fn repo_url(&self, job: &JobData) -> String {
        match &self.backend {
            RepositoryData::Rest {
                rest_url: restic_url,
                server_pubkey_file,
            } => {
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
                url
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub enum RepositoryData {
    /// Restic rest-server backend
    Rest {
        /// Repostiroy URL of the rest server.
        /// Does not contain the repo or user/password.
        rest_url: String,
        /// Pubkey for the server when HTTPS is used.
        server_pubkey_file: Option<PathBuf>,
    },
}

#[derive(Debug, Deserialize)]
pub struct JobData {
    /// For referencing jobs in commands and output
    pub name: String,
    /// Command to run pre backup
    pub pre_command: Option<String>,
    /// Paths to include for backup
    pub paths: Vec<PathBuf>,
    /// Exclude items see [restic docs](https://restic.readthedocs.io/en/latest/040_backup.html#excluding-files)
    pub excludes: Vec<String>,
    /// Repository/Bucket
    pub repository: String,
    /// Login user
    pub user: String,
    /// Password for user
    pub password: String,
    /// Encryption key
    pub repository_key: String,
    /// Command to run post backup
    pub post_command: Option<String>,
    /// Whether to run the post_command even on backup failure
    pub post_command_on_failure: bool,
    /// Interval in which to perform the backup
    pub interval: Option<u64>,
}
