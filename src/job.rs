use std::{process::Command, path::PathBuf, time::Duration};

use miette::{bail, Context, IntoDiagnostic, Result};
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;

use crate::config;
use crate::config::Global;


#[serde_as]
#[derive(Debug, Deserialize)]
pub struct Job {
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
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    pub interval: Option<Duration>,
}

impl Job {
    pub fn snapshots(&self, cfg: &Global) -> Result<Snapshots> {
        let mut cmd = self.command_base(cfg,"snapshots")?;
        let output = cmd.output().into_diagnostic()?;
        if output.stdout.starts_with(b"Fatal") ||!output.status.success() {
            if !output.stdout.is_empty() {
                eprintln!("RESTIC [{}]: {}",self.name,String::from_utf8_lossy(&output.stdout).trim());
            }
            if !output.stderr.is_empty() {
                eprintln!("RESTIC [{}]: {}",self.name,String::from_utf8_lossy(&output.stderr).trim());
            }
            bail!("Restic exited with errors, status code {:?}",output.status.code());
        }
        let res: Snapshots = serde_json::from_slice(&output.stdout)
            .into_diagnostic()?;
        Ok(res)
    }

    fn command_base(&self, cfg: &Global, command: &'static str) -> Result<Command> {
        let mut outp = Command::new(&cfg.restic_binary);
        outp.arg(command)
            .arg("--json")
            .arg("-q");
        match &cfg.backend {
            config::RepositoryData::Restic { restic_url: _, server_pubkey_file } => {
                outp.env("RESTIC_PASSWORD", self.repository_key.as_str())
                .env("RESTIC_REPOSITORY", cfg.repo_url(self)?);
                if let Some(key_file) = server_pubkey_file {
                    outp.arg("--cacert")
                    .arg(key_file);
                }
            },
        }
        Ok(outp)
    }
}

pub type Snapshots = Vec<Snapshot>;

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    pub time: String,
    pub paths: Vec<String>,
    pub hostname: String,
    pub username: String,
    pub id: String,
}
