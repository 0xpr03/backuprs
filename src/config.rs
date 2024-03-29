use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{remove_dir, DirBuilder};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use crate::error::{ComRes, CommandError};
use crate::job::Job;
use crate::job::JobMap;
use miette::{bail, Result};
use miette::{Context, IntoDiagnostic};
use serde::Deserialize;
use serde::Deserializer;
use serde::{de, Serialize};
use time::format_description;

#[derive(Debug, Deserialize, Default, Serialize)]
pub struct Conf {
    pub global: Global,
    /// All backup jobs
    pub job: Vec<JobData>,
}

impl Conf {
    pub fn split(self) -> Result<(Defaults, JobMap)> {
        self.global.check()?;
        let defaults = Rc::new(self.global);
        let mut jobs = HashMap::with_capacity(self.job.len());
        for job_data in self.job.into_iter() {
            let name = job_data.name.clone();
            let job = Job::new(job_data, defaults.clone())?;
            if let Some(old_job) = jobs.insert(name, job) {
                bail!(
                    "Multiple jobs with the same name '{}' detected!",
                    old_job.name()
                );
            }
        }

        Ok((defaults, jobs))
    }
}

pub type Defaults = Rc<Global>;

#[derive(Debug, Deserialize, Default, Serialize)]
pub struct Global {
    // Repository backends and defaults
    /// Rest backend defaults
    #[serde(alias = "REST", alias = "Rest")]
    pub rest: Option<RestRepository>,
    /// SFTP backend defaults
    #[serde(alias = "Sftp", alias = "SFTP")]
    pub sftp: Option<SftpRepository>,
    /// S3 backend defaults
    #[serde(alias = "S3")]
    pub s3: Option<S3Repository>,
    /// Path to restic binary
    pub restic_binary: PathBuf,
    /// Verbose output, passed via CLI params.  
    /// Value [0-3] for disabled to maximum level.
    #[serde(default)]
    pub verbose: usize,
    /// Default interval to use for backup jobs
    pub default_interval: u64,
    /// Period of time to perform backup jobs
    pub period: Option<BackupTimeRange>,
    /// Mysql Dump Path
    pub mysql_dump_binary: Option<PathBuf>,
    /// Postgres Dump Path
    pub postgres_dump_binary: Option<PathBuf>,
    /// Path for folder used for DB backups
    pub scratch_dir: PathBuf,
    #[serde(default)]
    pub verified_mysql_binary: Cell<bool>,
    #[serde(default)]
    pub verified_postgres_binary: Cell<bool>,
    #[serde(default = "default_true")]
    pub progress: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BackupTimeRange {
    /// Backup time start
    #[serde(deserialize_with = "deserialize_time")]
    pub backup_start_time: time::Time,
    /// Backup time end
    #[serde(deserialize_with = "deserialize_time")]
    pub backup_end_time: time::Time,
}

impl Default for BackupTimeRange {
    fn default() -> Self {
        Self {
            backup_start_time: time::Time::MIDNIGHT,
            backup_end_time: time::Time::MIDNIGHT,
        }
    }
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
        // test we can write to scratch_dir
        let scratch_test_dir = self.scratch_dir.join("testing");
        if scratch_test_dir.exists() {
            eprintln!(
                "Path for testing scratch dir write perms exists!\nPath {:?}",
                scratch_test_dir
            );
        } else {
            let mut builder = DirBuilder::new();
            builder.recursive(true);
            if let Err(e) = builder.create(&scratch_test_dir) {
                bail!(
                    "Failed to create scratch_dir test folder recursively at {:?}: {:?}",
                    scratch_test_dir,
                    e
                );
            }
            if let Err(e) = remove_dir(&scratch_test_dir) {
                bail!(
                    "Failed to delete scatch_dir test folder again at {:?}: {:?}",
                    scratch_test_dir,
                    e
                );
            }
        }

        if let Some(period) = &self.period {
            if period.backup_start_time == period.backup_end_time {
                bail!("Backup period start and end time can't be the same!");
            }
        }
        if let Some(path) = &self.mysql_dump_binary {
            if !path.is_file() {
                bail!("Path for config value 'mysql_dump_binary' is not an exsiting file!");
            }
        }
        if let Some(path) = &self.postgres_dump_binary {
            if !path.is_file() {
                bail!("Path for config value 'postgres_dump_binary' is not an exsiting file!");
            }
        }
        if let Some(RestRepository {
            rest_host: _,
            server_pubkey_file,
            rest_user: _,
            rest_password: _,
        }) = &self.rest
        {
            if let Some(pubkey_file) = server_pubkey_file {
                if !pubkey_file.exists() {
                    bail!("Default Rest 'server_pubkey_file' specified, but file does not exist?");
                }
                std::fs::File::open(&pubkey_file)
                    .into_diagnostic()
                    .wrap_err(
                        "Default Rest 'server_pubkey_file' specified, but can't read file?",
                    )?;
            }
        }
        Ok(())
    }
    pub fn mysql_cmd_base(&self) -> Command {
        if let Some(path) = &self.mysql_dump_binary {
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
        let binary = match &self.postgres_dump_binary {
            Some(path) => path.as_os_str(),
            None => {
                #[cfg(target_os = "windows")]
                {
                    OsStr::new("pg_dump.exe")
                }
                #[cfg(not(target_os = "windows"))]
                {
                    OsStr::new("pg_dump")
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

#[derive(Debug, Deserialize, Default, Serialize)]
/// Defaults for rest backend
pub struct RestRepository {
    /// Repostiroy host of the rest server. For example 10.0.0.1:443
    /// Does not contain the repo or user/password.
    pub rest_host: Option<String>,
    /// Pubkey for the server when HTTPS is used.
    pub server_pubkey_file: Option<PathBuf>,
    pub rest_user: Option<String>,
    pub rest_password: Option<String>,
}

macro_rules! impl_required_getters {
    ( $target:ident, $name:ident ) => {
        impl $target {
            pub fn $name<'a>(&'a self, defaults: &'a Option<$target>) -> ComRes<&'a str> {
                self.$name
                    .as_deref()
                    .or(defaults.as_ref().map(|v| v.$name.as_deref()).flatten())
                    .ok_or(CommandError::MissingConfigValue("$name"))
            }
        }
    };
}
macro_rules! impl_optional_getters {
    ( $target:ident, $name:ident, $ret_type:ty ) => {
        impl $target {
            pub fn $name<'a>(&'a self, defaults: &'a Option<$target>) -> Option<&'a $ret_type> {
                self.$name
                    .as_deref()
                    .or(defaults.as_ref().map(|v| v.$name.as_deref()).flatten())
            }
        }
    };
}

impl_required_getters!(RestRepository, rest_user);
impl_required_getters!(RestRepository, rest_password);
impl_required_getters!(RestRepository, rest_host);
impl_optional_getters!(RestRepository, server_pubkey_file, Path);

#[derive(Debug, Deserialize, Default, Serialize)]
/// Defaults for S3 backend
pub struct S3Repository {
    /// Host URL of the rest server.
    /// Does not contain the bucket or user/password.
    pub s3_host: Option<String>,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
}

impl_required_getters!(S3Repository, s3_host);
impl_required_getters!(S3Repository, aws_access_key_id);
impl_required_getters!(S3Repository, aws_secret_access_key);

#[derive(Debug, Deserialize, Default, Serialize)]
/// Defaults for rest backend
pub struct SftpRepository {
    /// Host URL of the sftp server.
    /// Does not contain the repo or user/password!
    pub sftp_host: Option<String>,
    /// Command for connecting.
    /// For `-o sftp.command="ssh -p 22 u1234@u1234.example.com -s sftp"`
    pub sftp_command: Option<String>,
    pub sftp_user: Option<String>,
}

impl_required_getters!(SftpRepository, sftp_host);
impl_optional_getters!(SftpRepository, sftp_command, str);
impl_required_getters!(SftpRepository, sftp_user);

#[derive(Debug, Deserialize, Default, Serialize)]
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
    #[serde(default)]
    pub post_command_on_failure: Option<bool>,
    /// Interval in which to perform the backup
    pub interval: Option<u64>,
    /// MySQL database name to backup
    pub mysql_db: Option<String>,
    /// Postgres database name to backup
    pub postgres_db: Option<PostgresData>,
}

/// Pre/Post user supplied command
#[derive(Debug, Deserialize, Default, Serialize)]
pub struct CommandData {
    pub command: String,
    pub args: Vec<String>,
    pub workdir: PathBuf,
}
/// Postgres backup data
#[derive(Debug, Deserialize, Default, Serialize)]
pub struct PostgresData {
    #[serde(default)]
    pub change_user: bool,
    pub password: Option<String>,
    pub user: Option<String>,
    pub database: String,
}

/// Per job backend
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "job_type")]
pub enum JobBackend {
    #[serde(alias = "s3")]
    S3(S3Repository),
    #[serde(alias = "rest", alias = "REST")]
    Rest(RestRepository),
    #[serde(alias = "sftp", alias = "Sftp")]
    SFTP(SftpRepository),
}

impl Default for JobBackend {
    fn default() -> Self {
        JobBackend::S3(Default::default())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn default_config() -> Conf {
        let mut conf = Conf::default();
        conf.global.s3 = Some(S3Repository::default());
        conf.global.rest = Some(RestRepository::default());
        conf.global.sftp = Some(SftpRepository::default());
        conf.job.push(Default::default());
        conf.job.push(Default::default());
        conf
    }

    #[test]
    fn test_default_config() {
        let conf = default_config();
        println!("{}", toml::to_string_pretty(&conf).unwrap());

        let config = include_str!("../config.toml.example");
        let _config: Conf = toml::from_str(config).unwrap();
    }

    #[test]
    #[ignore]
    fn test_default_config_verify() {
        let config = include_str!("../config.toml.example");
        let config: Conf = toml::from_str(config).unwrap();
        // requires working restic binary and pubkey file
        config.split().unwrap();
    }
}
