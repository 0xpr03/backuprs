use std::collections::HashMap;
use std::process::Output;
use std::rc::Rc;
use std::{path::PathBuf, process::Command, time::Duration};
use miette::{bail, Context, IntoDiagnostic, Result};
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;

use crate::config::{self, JobData};
use crate::config::Global;
use crate::error::{ComRes, CommandError};

pub type JobMap = HashMap<String,Job>;

pub struct Job {
    data: JobData,
    globals: Rc<Global>,
    /// whether this repo got intialized
    /// 
    /// false means the state is unknown
    is_initialized: bool,
}

impl Job {
    pub fn new(data: JobData, global: Rc<Global>) -> Self {
        Self {
            data,
            globals: global,
            is_initialized: false,
        }
    }
    #[inline]
    pub fn name(&self) -> &str {
        &self.data.name
    }
    pub fn backup(&mut self) -> Result<()> {
        if !self.is_initialized {
            if self.snapshots() == Err(CommandError::NotInitialized) {
                if self.globals.verbose {
                    println!("[{}] not initialized",self.name());
                }
                self.restic_init()?;
            }
        }

        let mut cmd = self.command_base("backup")?;

        for exclude in self.data.excludes.iter() {
            cmd.args(["-e",exclude.as_str()]);
        }
        // has to be last
        cmd.args(&self.data.paths);
        let output = cmd.output().into_diagnostic()?;
        self.check_errors(&output)?;
        

        Ok(())
    }

    #[inline]
    fn verbose(&self) -> bool {
        self.globals.verbose
    }

    pub fn restic_init(&mut self) -> Result<()> {
        if self.verbose() {
            println!("[{}] initializing repository",self.name());
        }
        let mut cmd = self.command_base("init")?;
        let output = cmd.output().into_diagnostic()?;
        self.check_errors(&output)?;
        // println!("{}",String::from_utf8(output.stdout).unwrap());
        // let res: Snapshots = serde_json::from_slice(&output.stdout).into_diagnostic()?;
        self.is_initialized = true;
        Ok(())
    }

    fn check_errors(&self, output: &Output) -> ComRes<()> {
        if output.stdout.starts_with(b"Fatal") || !output.status.success() {
            if !output.stderr.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Fatal: unable to open config file")
                    && stderr.contains("<config/> does not exist") {
                    if self.verbose() { // still print on verbose
                        self.print_output_verbose(output);
                    }
                    return Err(CommandError::NotInitialized);
                }
            }
            self.print_output_verbose(output);
            return Err(CommandError::ResticError(format!("status code {:?}",output.status.code())));
        }
        if self.verbose() {
            self.print_output_verbose(output);
        }
        Ok(())
    }

    fn print_output_verbose(&self, output: &Output) {
        if !output.stdout.is_empty() {
            let r_output = String::from_utf8_lossy(&output.stdout);
            for line in r_output.trim().lines() {
                eprintln!(
                    "[{}] RESTIC > {}",
                    self.data.name,
                    line
                );
            }
        }
        if !output.stderr.is_empty() {
            let r_output = String::from_utf8_lossy(&output.stderr);
            for line in r_output.trim().lines() {
                eprintln!(
                    "[{}] RESTIC > {}",
                    self.data.name,line
                );
            }
        }
    }

    pub fn snapshots(&mut self) -> ComRes<Snapshots> {
        let mut cmd = self.command_base("snapshots")?;
        let output = cmd.output()?;
        self.check_errors(&output)?;
        let res: Snapshots = serde_json::from_slice(&output.stdout)?;
        self.is_initialized = true;
        Ok(res)
    }

    fn command_base(&self, command: &'static str) -> ComRes<Command> {
        let mut outp = Command::new(&self.globals.restic_binary);
        outp.args([command,"--json","-q"]);
        match &self.globals.backend {
            config::RepositoryData::Rest {
                rest_url: _,
                server_pubkey_file,
            } => {
                outp.env("RESTIC_PASSWORD", self.data.repository_key.as_str())
                    .env("RESTIC_REPOSITORY", self.globals.repo_url(&self.data));
                if let Some(key_file) = server_pubkey_file {
                    outp.arg("--cacert").arg(key_file);
                }
            }
        }
        Ok(outp)
    }
}

pub type Snapshots = Vec<Snapshot>;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Snapshot {
    pub time: String,
    pub paths: Vec<String>,
    pub hostname: String,
    pub username: String,
    pub id: String,
}
