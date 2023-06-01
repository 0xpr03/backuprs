use miette::{bail, Context};
use miette::{miette, IntoDiagnostic, Result};
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::BufRead;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ChildStderr;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Instant;
use time::{Duration, OffsetDateTime};

use crate::config::{self, JobData};
use crate::config::{CommandData, Global};
use crate::error::{ComRes, CommandError};
use crate::models::*;

pub type JobMap = HashMap<String, Job>;

pub struct Job {
    data: JobData,
    globals: Rc<Global>,
    /// Last snapshot run
    ///
    /// also tells whether this repo got initialized
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
        job.verify()
            .wrap_err(miette!("[{}] Failed to load job configuration"))?;
        Ok(job)
    }

    fn verify(&self) -> Result<()> {
        if self.data.post_command.is_some() && self.data.post_command_on_failure.is_none() {
            bail!("Option 'post_command' is specified, but not 'post_command_on_failure'!");
        }
        match &self.data.backend {
            config::JobBackend::S3(s3) => {
                s3.aws_access_key_id(&self.globals.s3)?;
                s3.aws_secret_access_key(&self.globals.s3)?;
                s3.s3_host(&self.globals.s3)?;
            }
            config::JobBackend::Rest(rest) => {
                rest.rest_host(&self.globals.rest)?;
                rest.rest_password(&self.globals.rest)?;
                rest.rest_user(&self.globals.rest)?;
                let pubkey_file = rest.server_pubkey_file(&self.globals.rest);
                if self.verbose() {
                    match pubkey_file.is_some() {
                        true => println!("[{}] Server pubkey file found, using https", self.name()),
                        false => {
                            println!("[{}] No server pubkey file found, using http", self.name())
                        }
                    }
                }
                if let Some(pubkey_file) = pubkey_file {
                    if !pubkey_file.exists() {
                        bail!("Rest 'server_pubkey_file' specified, but file does not exist?");
                    }
                    std::fs::File::open(&pubkey_file)
                        .into_diagnostic()
                        .wrap_err(
                            "Default Rest 'server_pubkey_file' specified, but can't read file?",
                        )?;
                }
            }
            config::JobBackend::SFTP(sftp) => {
                sftp.sftp_host(&self.globals.sftp)?;
                sftp.sftp_user(&self.globals.sftp)?;
                match sftp.sftp_command(&self.globals.sftp).is_some() {
                    true => println!("[{}] Sftp connect command specified.", self.name()),
                    false => println!("[{}] No sftp connect command specified.", self.name()),
                }
            }
        }
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
    fn last_run_update(&self, last_run: Option<OffsetDateTime>) {
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
    pub fn update_last_run(&self) -> ComRes<()> {
        self.snapshots(Some(1)).map(|_| ())
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.data.name
    }

    /// Perform dry run with verbose information
    pub fn dry_run(&mut self) -> Result<()> {
        println!("[{}]\tStarting dry run", self.name());
        self.inner_backup(true)?;
        Ok(())
    }

    fn inner_backup(&self, dry_run: bool) -> Result<BackupSummary> {
        let mut context = BackupContext::new(&self.data, &self.globals.scratch_dir);
        let res = self._inner_backup(&mut context, dry_run);
        if let Err(e) = self.run_post_jobs(&mut context) {
            // don't overwrite the backup error
            if res.is_err() {
                eprintln!("Failed to perform post-jobs: {}", e);
            } else {
                return Err(e);
            }
        }
        drop(context);
        res
    }

    /// If dry_run run is set, performs it with verbose information
    fn _inner_backup(&self, context: &mut BackupContext, dry_run: bool) -> Result<BackupSummary> {
        self.assert_initialized()?;

        self.run_pre_jobs(context)?;

        let mut cmd = self.command_base("backup", false)?;

        if dry_run {
            cmd.args(["--verbose", "--dry-run"]);
        } else if self.verbose() {
            cmd.arg("--verbose");
        }
        for exclude in self.data.excludes.iter() {
            cmd.args(["-e", exclude.as_str()]);
        }
        // backup paths have to be last
        cmd.args(context.backup_paths());

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

        // cache, no Rc overhead
        let verbose = self.globals.verbose;
        let stats = self.globals.progress;
        let name = self.name();

        let mut backup_summary: Option<BackupSummary> = None;
        let mut last_progress = 0;
        let mut last_update = Instant::now();
        for line in bufreader.lines().filter_map(|l| l.ok()) {
            let line = line.trim();
            self.check_error_stdout(line)?;
            let msg: BackupMessage = serde_json::from_str(line).into_diagnostic()?;
            match msg {
                BackupMessage::VerboseStatus(v) => {
                    if dry_run || verbose > 1 {
                        match v.action.as_str() {
                            "unchanged" => println!("[{}]\tUnchanged \"{}\"", name, v.item),
                            "new" => {
                                let (unit, size) = format_size(v.data_size);
                                println!("[{}]\tNew \"{}\" {} {}", name, v.item, size, unit);
                            }
                            "changed" => {
                                let (unit, size) = format_size(v.data_size);
                                println!("[{}]\tNew \"{}\" {} {}", name, v.item, size, unit);
                            }
                            v => eprintln!("Unknown restic action '{}'", v),
                        }
                    }
                }
                BackupMessage::Status(status) => {
                    if stats {
                        match status {
                            BackupStatus::Finish(_) => (),
                            BackupStatus::Intermediate(s) => {
                                if last_update.elapsed() > Duration::seconds(1) {
                                    let percent: i32 = (s.percent_done * 100.0) as _;
                                    if percent != last_progress {
                                        last_progress = percent;
                                        println!(
                                            "[{}]\tBackup {}% finished, {} files finished",
                                            self.name(),
                                            percent,
                                            s.files_done
                                        );
                                        last_update = Instant::now();
                                    }
                                }
                            }
                        }
                    }
                }
                BackupMessage::Summary(s) => {
                    backup_summary = Some(s);
                }
            }
        }
        let status = handle.wait().into_diagnostic()?;

        self.check_errors_stderr(stderr, status)?;

        let summary = match backup_summary {
            Some(v) => Ok(v),
            None => bail!("No backup summary received from restic"),
        };
        // post_jobs run by context
        context.set_successfull();
        summary
    }

    /// Make sure the repo is initialized
    fn assert_initialized(&self) -> Result<()> {
        if self.last_run.get().is_none() {
            if self.update_last_run() == Err(CommandError::NotInitialized) {
                if self.globals.verbose > 0 {
                    println!("[{}] not initialized", self.name());
                }
                self.restic_init()?;
            }
        }
        Ok(())
    }

    fn run_pre_jobs(&self, context: &mut BackupContext) -> Result<()> {
        if let Some(mysql_db) = self.data.mysql_db.as_deref() {
            if self.verbose() {
                println!("[{}] Starting mysql dump", self.name());
            }
            let path = context.temp_dir()?;
            let dump_path = path.join("db_dump_mysql.sql");
            let mut args_output = OsString::from("--result-file=");
            args_output.push(&dump_path);

            let output = self
                .globals
                .mysql_cmd_base()
                .args(["--databases", mysql_db])
                .arg(args_output)
                .output()
                .into_diagnostic()
                .wrap_err("Starting mysqldump")?;
            if !output.status.success() {
                self.print_output_verbose(&output, "mysqldump");
                bail!(
                    "Mysqldump failed, exit code {}",
                    output.status.code().unwrap_or(0)
                )
            } else if self.verbose() {
                self.print_output_verbose(&output, "mysqldump");
            }
            context.register_backup_target(dump_path);
        }
        if let Some(postgres_db) = &self.data.postgres_db {
            if self.verbose() {
                println!("[{}] Starting postgres dump", self.name());
            }
            let path = context.temp_dir()?;
            let dump_path = path.join("db_dump_postgres.sql");
            let mut args_output = OsString::from("--file=");
            args_output.push(&dump_path);

            let mut cmd = self.globals.postgres_cmd_base(postgres_db.change_user)?;

            if let Some(user) = postgres_db.user.as_deref() {
                cmd.env("PGUSER", user);
            }
            if let Some(password) = postgres_db.password.as_deref() {
                // TODO: only safe on linux ?
                cmd.env("PGPASSWORD", password);
            }

            cmd.arg(args_output)
                // has to be last
                .arg(&postgres_db.database);

            if self.verbose() {
                println!("[{}] CMD: {:?}", self.name(), cmd);
            }
            let output = cmd
                .output()
                .into_diagnostic()
                .wrap_err("Starting pg_dump")?;
            if !output.status.success() {
                self.print_output_verbose(&output, "pg_dump");
                bail!(
                    "pg_dump failed, exit code {}",
                    output.status.code().unwrap_or(0)
                )
            } else if self.verbose() {
                self.print_output_verbose(&output, "pg_dump");
            }
            context.register_backup_target(dump_path);
        }
        if let Some(command_data) = &self.data.pre_command {
            self.run_user_command(context, command_data, "pre-command", true)?;
        }
        Ok(())
    }

    /// Run user command.
    ///
    /// - `err_naming` Name of the command for error reporting purposes (`pre-command`)
    /// - `success` passed to the command as environment variable
    fn run_user_command(
        &self,
        context: &mut BackupContext,
        command: &CommandData,
        err_naming: &'static str,
        success: bool,
    ) -> Result<()> {
        let path = context.temp_dir()?;

        let delimiter = std::ffi::OsStr::new(";");
        let targets = self
            .data
            .paths
            .iter()
            .fold(OsString::new(), |mut acc, path| {
                if !acc.is_empty() {
                    acc.push(&delimiter);
                }
                acc.push(path);
                acc
            });
        let excludes = self.data.excludes.join(";");
        let output = Command::new(&command.command)
            .args(&command.args)
            .env("BACKUPRS_TEMP_FOLDER", path)
            .env("BACKUPRS_TARGETS", targets)
            .env("BACKUPRS_EXCLUDES", excludes)
            .env("BACKUPRS_JOB_NAME", self.name())
            .env("BACKUPRS_SUCCESS", success.to_string())
            .output()
            .into_diagnostic()
            .wrap_err_with(|| format!("spawning {err_naming}"))?;
        if !output.status.success() {
            self.print_output_verbose(&output, err_naming);
            bail!(
                "{err_naming} failed, exit code {}",
                output.status.code().unwrap_or(0)
            )
        } else if self.verbose() {
            self.print_output_verbose(&output, err_naming);
        }
        Ok(())
    }

    fn run_post_jobs(&self, context: &mut BackupContext) -> Result<()> {
        if let Some(command_data) = &self.data.post_command {
            let post_command_on_failure = self.data.post_command_on_failure.ok_or_else(||miette!("Expected option 'post_command_on_failure'!"))?;
            if post_command_on_failure || context.success {
                self.run_user_command(context, command_data, "post-command", context.success)?;
            }
        }
        Ok(())
    }

    /// Run backup. Prints start and end. Does not check for correct duration to previous run.
    pub fn backup(&mut self) -> Result<BackupSummary> {
        println!("[{}]\tStarting backup", self.name());
        let summary = self.inner_backup(false)?;
        println!("[{}]\tBackup finished. {}", self.name(), summary);
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
        self.globals.verbose > 0
    }

    /// Initialize restic repository
    pub fn restic_init(&self) -> Result<()> {
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
                if self.globals.verbose > 2 {
                    // still print on verbose
                    self.print_line_verbose_restic(line, false);
                }
                return Err(CommandError::NotInitialized);
            }
            self.print_line_verbose_restic(line, false);
            return Err(CommandError::ResticError(String::new()));
        }
        if self.globals.verbose > 2 {
            self.print_line_verbose_restic(line, false);
        }
        Ok(())
    }

    /// Check restic respone for errors
    fn check_errors(&self, output: &Output) -> ComRes<()> {
        if output.stdout.starts_with(b"Fatal") || !output.status.success() {
            if !output.stderr.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Fatal: unable to open config file") {
                    if stderr.contains("<config/> does not exist") // rest
                     || stderr.contains("file does not exist") // sftp
                     || stderr.contains("Stat: The specified key does not exist")
                    // S3
                    {
                        if self.verbose() {
                            // still print on verbose
                            self.print_output_verbose_restic(output);
                        }
                        return Err(CommandError::NotInitialized);
                    }
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
    pub fn snapshots(&self, amount: Option<usize>) -> ComRes<Snapshots> {
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
        let mut outp: Command = Command::new(&self.globals.restic_binary);
        outp.args([command, "--json"]);
        if quiet {
            outp.arg("-q");
        }
        match &self.data.backend {
            config::JobBackend::Rest(rest_data) => {
                let mut url: String = String::from("rest:");
                let key_file = rest_data.server_pubkey_file(&self.globals.rest);
                if key_file.is_some() {
                    url.push_str("https://");
                } else {
                    url.push_str("http://");
                }
                url.push_str(&rest_data.rest_user(&self.globals.rest)?);
                url.push_str(":");
                url.push_str(&rest_data.rest_password(&self.globals.rest)?);
                url.push_str("@");
                url.push_str(rest_data.rest_host(&self.globals.rest)?);
                // match &rest_data.overrides {
                //     Some(overrides) => url.push_str(&overrides.rest_host),
                //     None => url.push_str(&default.rest_host),
                // }
                url.push_str("/");
                url.push_str(&self.data.repository);

                outp.env("RESTIC_PASSWORD", self.data.repository_key.as_str())
                    .env("RESTIC_REPOSITORY", url);
                if let Some(key_file) = key_file {
                    outp.arg("--cacert").arg(key_file);
                }
            }
            config::JobBackend::S3(s3_data) => {
                let mut url: String = String::from("s3:");
                url.push_str(&s3_data.s3_host(&self.globals.s3)?);
                url.push_str("/");
                url.push_str(&self.data.repository);

                outp.env("RESTIC_REPOSITORY", url)
                    .env("RESTIC_PASSWORD", self.data.repository_key.as_str())
                    .env(
                        "AWS_ACCESS_KEY_ID",
                        &s3_data.aws_access_key_id(&self.globals.s3)?,
                    )
                    .env(
                        "AWS_SECRET_ACCESS_KEY",
                        &s3_data.aws_secret_access_key(&self.globals.s3)?,
                    );
            }
            config::JobBackend::SFTP(sftp_data) => {
                let mut url: String = String::from("sftp:");
                let sftp_user = &sftp_data.sftp_user(&self.globals.sftp)?;
                url.push_str(sftp_user);
                url.push_str("@");
                let host: &str = sftp_data.sftp_host(&self.globals.sftp)?;
                url.push_str(&host);
                url.push_str(":/");
                url.push_str(&self.data.repository);

                if self.verbose() {
                    println!("[{}] Repo URL: '{url}'", self.name());
                }

                let connect_command = sftp_data.sftp_command(&self.globals.sftp);
                if let Some(command) = connect_command {
                    // -o sftp.command="foobar"
                    let connection_option = format!("sftp.command={command}")
                        .replace("{user}", sftp_user)
                        .replace("{host}", host);
                    if self.verbose() {
                        println!(
                            "[{}] Option sftp.command: '{connection_option}'",
                            self.name()
                        );
                    }
                    outp.args(["-o", &connection_option]);
                }

                outp.env("RESTIC_REPOSITORY", url)
                    .env("RESTIC_PASSWORD", self.data.repository_key.as_str());
            }
        }
        Ok(outp)
    }
}

// /// Guard container, for example containing cleanup jobs to perform on drop
// struct Guards(Vec<Box<dyn std::any::Any>>);

struct BackupContext<'a> {
    /// temporary directory to be used for additional operations
    ///
    /// Not backed up, but removed on job end
    temp_dir: Option<PathBuf>,
    /// Whether this job has had no errors.
    /// Used for post-command evaluation.
    success: bool,
    /// additional backup targets
    backup_targets: Vec<Cow<'a, Path>>,
    /// Base for creating a temporary directory
    temp_dir_base: &'a Path,
    job: &'a JobData,
}

impl Drop for BackupContext<'_> {
    fn drop(&mut self) {
        if let Some(path) = &self.temp_dir {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}

impl<'a> BackupContext<'a> {
    pub fn new(job: &'a JobData, temp_dir_base: &'a Path) -> Self {
        let mut context = Self {
            temp_dir: None,
            success: false,
            backup_targets: Vec::with_capacity(2),
            temp_dir_base,
            job,
        };
        let mut paths: Vec<Cow<'a, Path>> = job
            .paths
            .iter()
            .map(|v| Cow::Borrowed(v.as_path()))
            .collect();
        context.backup_targets.append(&mut paths);
        context
    }

    /// Get path for temporary directory
    pub fn temp_dir(&mut self) -> Result<&Path> {
        // TODO: use get_or_insert_default when stabilized
        // self.temp_dir.get_or_insert_default().path()
        if let None = self.temp_dir.as_deref() {
            let path = self
                .temp_dir_base
                .join(format!("{}_scratchspace", self.job.name));
            if path.exists() {
                if !path.is_dir() {
                    bail!(
                        "Creating temporary scratchspace directory at {} failed, already a file?!",
                        path.display()
                    );
                }
            } else {
                std::fs::create_dir_all(&path)
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!("Creating scratchspace directory at {}", path.display())
                    })?;
            }
            self.temp_dir = Some(path);
        }
        Ok(self.temp_dir.as_deref().unwrap())
    }

    pub fn backup_paths(&self) -> Vec<&Path> {
        self.backup_targets.iter().map(|v| v.as_ref()).collect()
    }

    /// Add additional backup target
    pub fn register_backup_target(&mut self, path: PathBuf) {
        self.backup_targets.push(Cow::Owned(path));
    }

    /// Mark job as successful.
    fn set_successfull(&mut self) {
        self.success = true;
    }
}