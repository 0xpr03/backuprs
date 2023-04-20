use std::cell::Cell;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use miette::{bail, miette, Result};
use miette::{Context, IntoDiagnostic};
use serde::de;
use serde::Deserialize;
use serde::Deserializer;
use time::format_description;

use crate::error::{ComRes, CommandError};
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
    // Repository backends and defaults
    /// Rest backend defaults
    pub rest: Option<RestRepository>,
    /// SFTP backend defaults
    pub sftp: Option<SftpRepository>,
    /// S3 backend defaults
    pub s3: Option<S3Repository>,
    /// Path to restic binary
    pub restic_binary: PathBuf,
    /// Verbose output, passed via CLI params
    #[serde(default)]
    pub verbose: bool,
    /// Default interval to use for backup jobs
    pub default_interval: u64,
    /// Period of time to perform backup jobs
    pub period: Option<BackupTimeRange>,
    /// Mysql Dumb Path
    pub mysql_dumb_binary: Option<PathBuf>,
    /// Postgres Dumb Path
    pub postgres_dumb_binary: Option<PathBuf>,
    /// Path for folder used for DB backups
    pub scratch_dir: PathBuf,
    #[serde(default)]
    pub verified_mysql_binary: Cell<bool>,
    #[serde(default)]
    pub verified_postgres_binary: Cell<bool>,
    #[serde(default)]
    pub progress: bool,
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
        if !self.scratch_dir.is_dir() {
            bail!("Path for config value 'scratch_dir' is not an existing folder!");
        }
        if let Some(period) = &self.period {
            if period.backup_start_time == period.backup_end_time {
                bail!("Backup period start and end time can't be the same!");
            }
        }
        if let Some(path) = &self.mysql_dumb_binary {
            if !path.is_file() {
                bail!("Path for config value 'mysql_dumb_binary' is not an exsiting file!");
            }
        }
        if let Some(path) = &self.postgres_dumb_binary {
            if !path.is_file() {
                bail!("Path for config value 'postgres_dumb_binary' is not an exsiting file!");
            }
        }
        if let Some(RestRepository {
            rest_host: rest_url,
            server_pubkey_file,
        }) = &self.rest
        {
            if let Some(pubkey_file) = server_pubkey_file {
                if !pubkey_file.exists() {
                    bail!("Rest 'server_pubkey_file' specified, but file does not exist?");
                }
                std::fs::File::open(&pubkey_file)
                    .into_diagnostic()
                    .wrap_err("Rest 'server_pubkey_file' specified, but can't read file?")?;
            }
        }
        Ok(())
    }
    pub fn mysql_cmd_base(&self) -> Command {
        if let Some(path) = &self.mysql_dumb_binary {
            Command::new(path)
        } else {
            #[cfg(target_os = "windows")]
            let cmd = "mysqldump.exe";
            #[cfg(not(target_os = "windows"))]
            let cmd = "mysqldump";

            Command::new(cmd)
        }
    }
    pub fn postgres_cmd_base(&self, sudo: bool) -> Result<Command> {
        let binary = match &self.postgres_dumb_binary {
            Some(path) => path.as_os_str(),
            None => {
                #[cfg(target_os = "windows")]
                {
                    OsStr::new("pg_dumb.exe")
                }
                #[cfg(not(target_os = "windows"))]
                {
                    OsStr::new("pg_dumb")
                }
            }
        };
        match sudo {
            true => {
                #[cfg(target_os = "windows")]
                bail!("PostgreSQL user change (sudo) not supported on windows!");
                #[cfg(not(target_os = "windows"))]
                {
                    let mut command = Command::new("sudo");
                    command.arg("-u").arg("postgres").arg(binary);
                    Ok(command)
                }
            }
            false => Ok(Command::new(binary)),
        }
    }
}

// #[derive(Debug, Deserialize)]
// pub enum RepositoryData {
//     /// Restic rest-server backend
//     Rest {
//         /// Repostiroy URL of the rest server.
//         /// Does not contain the repo or user/password.
//         rest_url: String,
//         /// Pubkey for the server when HTTPS is used.
//         server_pubkey_file: Option<PathBuf>,
//     },
// }

#[derive(Debug, Deserialize)]
/// Defaults for rest backend
pub struct RestRepository {
    /// Repostiroy host of the rest server. For example 10.0.0.1:443
    /// Does not contain the repo or user/password.
    pub rest_host: String,
    /// Pubkey for the server when HTTPS is used.
    pub server_pubkey_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
/// Defaults for S3 backend
pub struct S3Repository {
    /// Host URL of the rest server.
    /// Does not contain the bucket or user/password.
    pub s3_host: String,
}

#[derive(Debug, Deserialize)]
/// Defaults for rest backend
pub struct SftpRepository {
    /// Host URL of the sftp server.
    /// Does not contain the repo or user/password!
    pub sftp_host: String,
    /// Command for connecting.
    /// For `-o sftp.command="ssh -p 22 u1234@u1234.example.com -s sftp"`
    pub sftp_command: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JobData {
    /// For referencing jobs in commands and output
    pub name: String,
    /// Command to run pre backup
    pub pre_command: Option<CommandData>,
    /// Paths to include for backup
    pub paths: Vec<PathBuf>,
    /// Exclude items see [restic docs](https://restic.readthedocs.io/en/latest/040_backup.html#excluding-files)
    pub excludes: Vec<String>,
    /// Repository / Bucket
    pub repository: String,
    /// Job Backend data
    #[serde(flatten)]
    pub backend: JobBackend,
    /// Encryption key
    pub repository_key: String,
    /// Command to run post backup
    pub post_command: Option<CommandData>,
    /// Whether to run the post_command even on backup failure
    pub post_command_on_failure: bool,
    /// Interval in which to perform the backup
    pub interval: Option<u64>,
    /// MySQL database name to backup
    pub mysql_db: Option<String>,
    /// Postgres database name to backup
    pub postgres_db: Option<PostgresData>,
}

/// Pre/Post user supplied command
#[derive(Debug, Deserialize)]
pub struct CommandData {
    pub command: String,
    pub args: Vec<String>,
    pub workdir: PathBuf,
}
/// Postgres backup data
#[derive(Debug, Deserialize)]
pub struct PostgresData {
    #[serde(default)]
    pub change_user: bool,
    pub password: Option<String>,
    pub user: Option<String>,
    pub database: String,
}

/// Per job backend
#[derive(Debug, Deserialize)]
pub enum JobBackend {
    S3(S3JobData),
    Rest(RestJobData),
    SFTP(SftpJobData),
}

/// Per job s3-backend data
#[derive(Debug, Deserialize)]
pub struct S3JobData {
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    #[serde(flatten)]
    pub overrides: Option<S3Repository>,
}

/// Per job rest-backend data
#[derive(Debug, Deserialize)]
pub struct RestJobData {
    pub rest_user: String,
    pub rest_password: String,
    #[serde(flatten)]
    pub overrides: Option<RestRepository>,
}

/// Per job sftp-backend data
#[derive(Debug, Deserialize)]
pub struct SftpJobData {
    pub sftp_user: String,
    #[serde(flatten)]
    pub overrides: Option<SftpRepository>,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = include_str!("../config.toml.example");
        let _config: Conf = toml::from_str(config).unwrap();
    }
}
