use std::{
    fs::File,
    io::{BufReader, Read},
    process::Command,
};

use clap::{Parser, Subcommand};
use config::{Conf, Global};
use miette::{bail, Context, IntoDiagnostic, Result};
use time::{OffsetDateTime, Time};

use crate::error::CommandError;

mod config;
mod error;
mod job;
mod models;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Verbose output
    #[arg(short, long, default_value_t = 0)]
    verbose: usize,
    /// Disable progress output for backups.
    #[arg(short, long, default_value_t = false)]
    no_progress: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Test config or perform dry-runs
    Test {
        /// Dry run, do not perform backup, only print what would happen.
        ///
        /// Equals `restic backup --dry-run`. Requires job argument.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Test specific job by name
        #[arg(short, long)]
        job: Option<String>,
    },
    /// Force run all or one backup job
    Run {
        /// Run specific job by name
        #[arg(short, long)]
        job: Option<String>,
        /// Abort on first error, stops any further jobs
        #[arg(short, long, default_value_t = false)]
        abort_on_error: bool,
    },
    /// Daemonize and run backups in specified intervals
    Daemon {},
}

// /// Turn debugging information on
// #[arg(short, long)]
// test: bool,

// /// Run specific job by name
// #[arg(short, long)]
// job: Option<String>,

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = read_config().wrap_err("Reading configuration")?;
    if cli.verbose > 0 {
        config.global.verbose = cli.verbose;
    }
    if cli.no_progress {
        config.global.progress = false;
    }

    config.global.check()?;
    check_restic(&config.global)?;
    // TODO: fail on duplicate job names
    let (defaults, mut jobs) = config.split()?;

    match &cli.command {
        Commands::Run {
            job,
            abort_on_error: _,
        } => {
            // maybe unify this here, but would require creating an ad-hoc iterator of
            // one element
            if let Some(jobname) = job {
                if let Some(job) = jobs.get_mut(jobname) {
                    match job.backup() {
                        Ok(_) => (),
                        Err(e) => {
                            eprintln!("[{}] Failed to backup.", job.name());
                            return Err(e);
                        }
                    }
                } else {
                    bail!("No job named '{}' found!", jobname);
                }
            } else {
                let mut run = 0;
                let mut failed = 0;
                for job in jobs.values_mut() {
                    match job.backup() {
                        Ok(_) => (),
                        Err(e) => {
                            failed += 1;
                            eprintln!("[{}]\tFailed to backup. {}", job.name(), e);
                        }
                    }
                    run += 1;
                }
                println!("Backup run finished. {}/{} jobs failed.", failed, run);
            }
        }
        Commands::Test { dry_run, job } => {
            let mut failed = 0;
            if *dry_run {
                match job {
                    Some(target_name) => {
                        println!("Dry run mode.");
                        for (name, job) in jobs.iter_mut() {
                            if name == target_name {
                                job.dry_run()?;
                                return Ok(());
                            }
                        }
                        bail!("No job named '{}' found!", target_name);
                    }
                    None => {
                        bail!("Dry run flag requires a job name!");
                    }
                }
            }
            match &defaults.period {
                Some(period) => {
                    let format =
                        time::format_description::parse("[hour]:[minute]").into_diagnostic()?;
                    let start_fmt = period.backup_start_time.format(&format).into_diagnostic()?;
                    let end_fmt = period.backup_end_time.format(&format).into_diagnostic()?;
                    println!("Backup period specified. Backups will only start between {} and {}  o'clock.",
                    start_fmt,end_fmt);
                }
                None => println!(
                    "No backup period specified. Jobs start when intervall timeout is reached."
                ),
            }
            // println!("Backup starting time is {}",defaults.backup_start_time);
            for (_, job) in jobs.iter_mut() {
                match job.update_last_run() {
                    Ok(_) => {
                        let next_run = job.next_run()?;
                        println!(
                        "[{}]\tJob ok, found snapshots, last backup {}, next backup would be at {}",
                        job.name(),
                        job.last_run().expect("Expected at least one snapshot"),
                        next_run,
                    );
                    }
                    Err(e) => {
                        if e == CommandError::NotInitialized {
                            println!("[{}]\tRepo not initialized?", job.name());
                        } else {
                            eprintln!("[{}]\tCheck failed: {}", job.name(), e);
                            failed += 1;
                        }
                    }
                }
            }
            if failed > 0 {
                eprintln!("Failed test for {} jobs", failed);
            } else {
                println!("Test successfull");
            }
        }
        Commands::Daemon {} => {
            // update last_run for each job
            if jobs.is_empty() {
                bail!("No backup jobs configured!");
            }
            println!("Loading job snapshots");
            let mut jobs: Vec<_> = jobs
                .into_values()
                .map(|v| {
                    let _ = v.snapshots(Some(1));
                    v
                })
                .collect();

            println!("Entering daemon mode");
            loop {
                jobs.sort_unstable_by(|a, b| b.next_run().unwrap().cmp(&a.next_run().unwrap()));

                if let Some(mut job) = jobs.pop() {
                    let now = OffsetDateTime::now_local().into_diagnostic()?;
                    let sleep_time = job.next_run()? - now;
                    // job interval
                    if sleep_time.is_positive() {
                        if defaults.verbose > 0 {
                            println!("Waiting for cooldown time of job [{}]", job.name());
                        }
                        std::thread::sleep(sleep_time.try_into().into_diagnostic()?);
                    }
                    // backup window
                    if let Some(period) = &defaults.period {
                        let now = OffsetDateTime::now_local().into_diagnostic()?;
                        if let Some(duration) =
                            calc_period_sleep(period.backup_start_time, period.backup_end_time, now)
                        {
                            if defaults.verbose > 0 {
                                println!("Waiting for backup start time");
                            }
                            std::thread::sleep(duration.try_into().into_diagnostic()?);
                        }
                    }
                    match job.backup() {
                        Ok(_) => (),
                        Err(e) => {
                            eprintln!("[{}]\tFailed to backup.", job.name());
                            return Err(e);
                        }
                    }
                    // refresh last update time before adding the job back to the list
                    if let Err(e) = job.update_last_run() {
                        eprintln!(
                            "[{}]\t Failed to refresh last update run! {}",
                            job.name(),
                            e
                        );
                    }

                    jobs.push(job);
                }
            }
        }
    }

    Ok(())
}

fn read_config() -> Result<Conf> {
    let file = File::open("config.toml").into_diagnostic()?;
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mt = file.metadata().into_diagnostic()?;
        let mode = mt.permissions().mode();
        if mode & 0o007 != 0 {
            bail!("Config file is world readable, aborting!");
        }
    }
    let mut reader = BufReader::new(file);
    let mut cfg = String::new();
    reader.read_to_string(&mut cfg).into_diagnostic()?;

    let config: Conf = toml::from_str(&cfg).into_diagnostic()?;
    Ok(config)
}

fn check_restic(cfg: &Global) -> Result<()> {
    let outp = Command::new(&cfg.restic_binary)
        .arg("version")
        // .arg("--json") // unsupported
        .output()
        .into_diagnostic()
        .wrap_err("Restic can't be started")?;
    if !outp.status.success() {
        bail!(
            "Restic exited test with status code {:?}",
            outp.status.code()
        );
    }
    if !outp.stdout.starts_with(b"restic") {
        bail!(
            "Restic binary returned invalid output: {} {}",
            String::from_utf8_lossy(&outp.stdout),
            String::from_utf8_lossy(&outp.stderr),
        );
    }

    Ok(())
}

fn calc_period_sleep(
    start: Time,
    end: Time,
    current_datetime: OffsetDateTime,
) -> Option<std::time::Duration> {
    let c_time = current_datetime.time();
    match end > start {
        // ex 06:00 - 18:00
        true => {
            if c_time < start {
                // ex 05:00
                return Some((start - c_time).try_into().unwrap());
            } else if c_time >= end {
                // ex 22:00
                let new_run = current_datetime.replace_time(start) + time::Duration::DAY;
                return Some((new_run - current_datetime).try_into().unwrap());
            }
            None
        }
        // ex 22:00 - 02:00
        false => {
            // ex 19:00
            if c_time < start && c_time >= end {
                return Some((start - c_time).try_into().unwrap());
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use time::Time;

    #[test]
    fn test_calc_period() {
        // 05:00-07:00
        let start = Time::from_hms(5, 0, 0).unwrap();
        let end = Time::from_hms(7, 0, 0).unwrap();
        let date_time = OffsetDateTime::now_local().unwrap();

        assert_eq!(
            None,
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(6, 0, 0).unwrap())
            )
        );
        assert_eq!(
            None,
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(5, 0, 0).unwrap())
            )
        );
        assert_eq!(
            Some(Duration::from_secs(60 * 60)),
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(4, 0, 0).unwrap())
            )
        );
        assert_eq!(
            Some(Duration::from_secs(60 * 60 * 22)),
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(7, 0, 0).unwrap())
            )
        );

        // 22:00-02:00
        let start = Time::from_hms(22, 0, 0).unwrap();
        let end = Time::from_hms(2, 0, 0).unwrap();

        assert_eq!(
            None,
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(23, 0, 0).unwrap())
            )
        );
        assert_eq!(
            None,
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(1, 0, 0).unwrap())
            )
        );
        assert_eq!(
            Some(Duration::from_secs(60 * 60)),
            calc_period_sleep(
                start,
                end,
                date_time.replace_time(Time::from_hms(21, 0, 0).unwrap())
            )
        );
    }
}
