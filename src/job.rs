use miette::{bail, Context};
use miette::{miette, IntoDiagnostic, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Display;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Stderr;
use std::path::{Path, PathBuf};
use std::process::ChildStderr;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::rc::Rc;
use temp_dir::TempDir;
use time::{Duration, OffsetDateTime};

use crate::config::Global;
use crate::config::{self, JobData};
use crate::error::{ComRes, CommandError};

pub type JobMap = HashMap<String, Job>;

pub struct Job {
    data: JobData,
    globals: Rc<Global>,
    /// Last snapshot run
    ///
    /// also tells whether this repo got intialized
    last_run: Cell<Option<OffsetDateTime>>,
    next_run: Cell<Option<OffsetDateTime>>,
}

impl Job {
    pub fn new(data: JobData, global: Rc<Global>) -> Result<Self> {
        let job = Self {
            data,
            globals: global,
            last_run: Cell::new(None),
            next_run: Cell::new(None),
        };
        job.verify()?;
        Ok(job)
    }

    fn verify(&self) -> Result<()> {
        // unused
        Ok(())
    }

    /// Time of last backup run
    pub fn last_run(&self) -> Option<OffsetDateTime> {
        self.last_run.get()
    }

    fn interval(&self) -> u64 {
        self.data.interval.unwrap_or(self.globals.default_interval)
    }

    /// Update last_run and invalidate next_run
    fn last_run_update(&mut self, last_run: Option<OffsetDateTime>) {
        self.last_run.set(last_run);
        self.next_run.set(None);
    }

    /// Time of next expected backup run
    pub fn next_run(&self) -> Result<OffsetDateTime> {
        if let Some(v) = self.next_run.get() {
            return Ok(v);
        }
        match self.last_run() {
            Some(last_run) => {
                let v = last_run
                    .checked_add(Duration::minutes(self.interval() as _))
                    .expect("overflow calculating next backup time!");
                self.next_run.set(Some(v));
                Ok(v)
            }
            None => OffsetDateTime::now_local().into_diagnostic(),
        }
    }

    /// Update last_run value by fetching latest snapshots.
    ///
    /// Can emit CommandError::NotInitialized.
    pub fn update_last_run(&mut self) -> ComRes<()> {
        self.snapshots(Some(1)).map(|_| ())
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.data.name
    }

    /// Perform dry run with verbose information
    pub fn dry_run(&mut self) -> Result<()> {
        println!("[{}]\tStarting dry run", self.name());
        self.backup_inner(true)?;
        Ok(())
    }

    /// Perform dry run with verbose information
    pub fn backup_inner(&mut self, dry_run: bool) -> Result<BackupSummary> {
        
        self.assert_initialized()?;

        let mut context = BackupContext::new();
        self.run_pre_jobs(&mut context)?;

        let mut cmd = self.command_base("backup", false)?;
        
        if dry_run {
            cmd.args(["--verbose","--dry-run"]);
        } else if self.verbose() {
            cmd.arg("--verbose");
        }
        for exclude in self.data.excludes.iter() {
            cmd.args(["-e", exclude.as_str()]);
        }
        // backup paths have to be last
        cmd.args(&self.data.paths);
        cmd.args(&context.backup_targets);

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut handle = cmd.spawn().into_diagnostic()?;

        let stdout = handle
            .stdout
            .take()
            .ok_or_else(|| miette!("Could not capture standard output."))?;
        let stderr = handle
            .stderr
            .take()
            .ok_or_else(|| miette!("Could not capture standard output."))?;
        let bufreader = BufReader::new(stdout);

        let verbose = self.verbose(); // cache, no Rc overhead
        let mut backup_summary: Option<BackupSummary> = None;

        for line in bufreader.lines().filter_map(|l| l.ok()) {
            let line = line.trim();
            self.check_error_stdout(line)?;
            let msg: BackupMessage = serde_json::from_str(line).into_diagnostic()?;
            match msg {
                BackupMessage::VerboseStatus(v) => {
                    if dry_run || verbose {
                    match v.action.as_str() {
                    "unchanged" => println!("[{}]\tUnchanged \"{}\"", self.name(), v.item),
                    "new" => {
                        let (unit, size) = format_size(v.data_size);
                        println!("[{}]\tNew \"{}\" {} {}", self.name(), v.item, size, unit);
                    }
                    "changed" => {
                        let (unit, size) = format_size(v.data_size);
                        println!("[{}]\tNew \"{}\" {} {}", self.name(), v.item, size, unit);
                    }
                    v => eprintln!("Unknown restic action '{}'", v),
                }}}
                ,
                BackupMessage::Status(_) => (),
                BackupMessage::Summary(s) => {
                    backup_summary = Some(s);
                }
            }
        }
        let status = handle.wait().into_diagnostic()?;

        self.check_errors_stderr(stderr, status)?;

        self.run_post_jobs(&mut context)?;

        drop(context);

        match backup_summary {
            Some(v) => Ok(v),
            None => bail!("No backup summary received from restic"),
        }
    }

    /// Make sure the repo is initialized
    fn assert_initialized(&mut self) -> Result<()> {
        if self.last_run.get().is_none() {
            if self.update_last_run() == Err(CommandError::NotInitialized) {
                if self.globals.verbose {
                    println!("[{}] not initialized", self.name());
                }
                self.restic_init()?;
            }
        }
        Ok(())
    }

    fn run_pre_jobs(&mut self, context: &mut BackupContext) -> Result<()> {
        if let Some(mysql_db) = self.data.mysql_db.as_deref() {
            let path = context.temp_dir()?;
            let dumb_path = path.join("db_dumb_mysql.sql");
            let mut arg = OsString::from("--result-file=");
            arg.push(&dumb_path);

            let output = self
                .globals
                .mysql_cmd_base()
                .args(["--databases", mysql_db])
                .arg(arg)
                .output()
                .into_diagnostic()
                .wrap_err("Starting mysqldumb")?;
            if !output.status.success() {
                self.print_output_verbose(&output, "mysqldumb");
                bail!(
                    "Mysqldumb failed, exit code {}",
                    output.status.code().unwrap_or(0)
                )
            } else if self.verbose() {
                self.print_output_verbose(&output, "mysqldumb");
            }
            context.register_backup_target(dumb_path);
        }
        if let Some(postgres_db) = self.data.postgres_db.as_deref() {
            let path = context.temp_dir()?;
            let dumb_path = path.join("db_dumb_postgres.sql");
            let mut arg = OsString::from("--file=");
            arg.push(&dumb_path);

            let output = self
                .globals
                .mysql_cmd_base()
                .arg(arg)
                .arg(postgres_db)
                .output()
                .into_diagnostic()
                .wrap_err("Starting pg_dumb")?;
            if !output.status.success() {
                self.print_output_verbose(&output, "pg_dumb");
                bail!(
                    "pg_dumb failed, exit code {}",
                    output.status.code().unwrap_or(0)
                )
            } else if self.verbose() {
                self.print_output_verbose(&output, "pg_dumb");
            }
            context.register_backup_target(dumb_path);
        }
        Ok(())
    }

    fn run_post_jobs(&mut self, context: &mut BackupContext) -> Result<()> {
        Ok(())
    }

    /// Run backup. Prints start and end. Does not check for correct duration to previous run.
    pub fn backup(&mut self) -> Result<BackupSummary> {
        println!("[{}]\tStarting backup", self.name());
        let summary = self.backup_inner(false)?;
        print!("[{}]\tBackup finished. {}", self.name(), summary);
        if self.verbose() {
            println!("[{}]\tBackup Details: {:?}", self.name(), summary);
        }
        Ok(summary)
    }

    /// Deserialize restic response or print all output on error
    fn des_response<T: DeserializeOwned>(&self, output: &Output) -> ComRes<T> {
        let res: T = match serde_json::from_slice(&output.stdout) {
            Ok(v) => v,
            Err(e) => {
                self.print_output_verbose_restic(output);
                return Err(e.into());
            }
        };
        Ok(res)
    }

    #[inline]
    fn verbose(&self) -> bool {
        self.globals.verbose
    }

    /// Initialize restic repository
    pub fn restic_init(&mut self) -> Result<()> {
        if self.verbose() {
            println!("[{}] \t initializing repository", self.name());
        }
        let mut cmd = self.command_base("init", true)?;
        let output = cmd.output().into_diagnostic()?;
        self.check_errors(&output)?;
        // println!("{}",String::from_utf8(output.stdout).unwrap());
        // let res: Snapshots = serde_json::from_slice(&output.stdout).into_diagnostic()?;
        self.snapshots(Some(1))?;
        Ok(())
    }

    /// Check for errors in stderr, for streaming commands
    fn check_errors_stderr(&self, stderr: ChildStderr, status: ExitStatus) -> ComRes<()> {
        let stderr = BufReader::new(stderr);
        for line in stderr.lines().filter_map(|l| l.ok()) {
            if line.trim().starts_with("Fatal") || !status.success() {
                if line.contains("Fatal: unable to open config file")
                    && line.contains("<config/> does not exist")
                {
                    if self.verbose() {
                        // still print on verbose
                        self.print_line_verbose_restic(&line, true);
                    }
                    return Err(CommandError::NotInitialized);
                }
                self.print_line_verbose_restic(&line, true);
                return Err(CommandError::ResticError(format!(
                    "status code {:?}",
                    status.code()
                )));
            }
            if self.verbose() {
                self.print_line_verbose_restic(&line, true);
            }
        }
        Ok(())
    }

    /// Check stdout line for errors, for streaming commands
    fn check_error_stdout(&self, line: &str) -> ComRes<()> {
        if line.starts_with("Fatal") {
            if line.contains("Fatal: unable to open config file")
                && line.contains("<config/> does not exist")
            {
                if self.verbose() {
                    // still print on verbose
                    self.print_line_verbose_restic(line, false);
                }
                return Err(CommandError::NotInitialized);
            }
            self.print_line_verbose_restic(line, false);
            return Err(CommandError::ResticError(String::new()));
        }
        if self.verbose() {
            self.print_line_verbose_restic(line, false);
        }
        Ok(())
    }

    /// Check restic respone for errors
    fn check_errors(&self, output: &Output) -> ComRes<()> {
        if output.stdout.starts_with(b"Fatal") || !output.status.success() {
            if !output.stderr.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Fatal: unable to open config file")
                    && stderr.contains("<config/> does not exist")
                {
                    if self.verbose() {
                        // still print on verbose
                        self.print_output_verbose_restic(output);
                    }
                    return Err(CommandError::NotInitialized);
                }
            }
            self.print_output_verbose_restic(output);
            return Err(CommandError::ResticError(format!(
                "status code {:?}",
                output.status.code()
            )));
        }
        if self.verbose() {
            self.print_output_verbose_restic(output);
        }
        Ok(())
    }

    #[inline]
    fn print_line_verbose_restic(&self, line: &str, stderr: bool) {
        self.print_line_verbose(line, "RESTIC", stderr);
    }

    #[inline]
    fn print_line_verbose(&self, line: &str, program: &'static str, stderr: bool) {
        if stderr {
            eprintln!("[{}]\t{}: {}", self.data.name, program, line);
        } else {
            println!("[{}]\t{}: {}", self.data.name, program, line);
        }
    }

    /// Helper to print restic cmd output for verbose flag
    fn print_output_verbose_restic(&self, output: &Output) {
        self.print_output_verbose(output, "RESTIC");
    }

    /// Helper to print restic cmd output for verbose flag
    fn print_output_verbose(&self, output: &Output, program: &'static str) {
        if !output.stdout.is_empty() {
            let r_output = String::from_utf8_lossy(&output.stdout);
            for line in r_output.trim().lines() {
                self.print_line_verbose(line, program, false);
            }
        }
        if !output.stderr.is_empty() {
            let r_output = String::from_utf8_lossy(&output.stderr);
            for line in r_output.trim().lines() {
                self.print_line_verbose(line, program, true);
            }
        }
    }

    /// Retrive snapshots for repo
    ///
    /// If checking for repository status, specify an amount
    ///
    /// Also sets last_run / initialized flag based on outcome
    pub fn snapshots(&mut self, amount: Option<usize>) -> ComRes<Snapshots> {
        let mut cmd = self.command_base("snapshots", true)?;
        if let Some(amount) = amount {
            cmd.args(["--latest", &amount.to_string()]);
        }

        let output = cmd.output()?;
        self.check_errors(&output)?;
        let snapshots: Snapshots = self.des_response(&output)?;
        if self.verbose() {
            println!("[{}]\t Snapshots: {:?}", self.name(), snapshots);
        }
        self.last_run_update(snapshots.last().map(|v| v.time));
        Ok(snapshots)
    }

    /// Restic command base
    fn command_base(&self, command: &'static str, quiet: bool) -> ComRes<Command> {
        let mut outp = Command::new(&self.globals.restic_binary);
        outp.args([command, "--json"]);
        if quiet {
            outp.arg("-q");
        }
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

// /// Guard container, for example containing cleanup jobs to perform on drop
// struct Guards(Vec<Box<dyn std::any::Any>>);

struct BackupContext {
    temp_dir: Option<TempDir>,
    backup_targets: Vec<PathBuf>,
}

impl BackupContext {
    pub fn new() -> Self {
        Self {
            temp_dir: None,
            backup_targets: Vec::with_capacity(2),
        }
    }

    /// Get path for temporary directory
    pub fn temp_dir(&mut self) -> Result<&Path> {
        // TODO: use get_or_insert_default when stabilized
        // self.temp_dir.get_or_insert_default().path()
        if let None = self.temp_dir {
            self.temp_dir = Some(TempDir::new().into_diagnostic()?);
        }
        Ok(self.temp_dir.as_ref().unwrap().path())
    }

    /// Add additional backup target
    pub fn register_backup_target(&mut self, path: PathBuf) {
        self.backup_targets.push(path);
    }
}

const fn format_size(bytes: usize) -> (&'static str, usize) {
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
