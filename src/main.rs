use std::{
    fs::File,
    io::{BufReader, Read},
    process::Command,
};

use clap::{Parser, Subcommand};
use config::{Global, Conf};
use job::Job;
use miette::{bail, Context, IntoDiagnostic, Result};

use crate::error::CommandError;

mod config;
mod job;
mod error;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Verbose output
    #[arg(short,long, default_value_t = false)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Test config
    Test { 
        /// Dry run, do not perform backup, only print what would happen.
        /// 
        /// Equals `restic backup --dry-run`
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
    Daemon {
    }
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
    if cli.verbose {
        config.global.verbose = true;
    }

    config.global.check()?;
    check_restic(&config.global)?;
    // TODO: fail on duplicate job names
    let (defaults, mut jobs) = config.split();

    match &cli.command {
        Commands::Run { job, abort_on_error } => {
            // maybe unify this here, but would require creating an ad-hoc iterator of
            // one element
            if let Some(jobname) = job {
                if let Some(job) = jobs.get_mut(jobname) {
                    match job.backup() {
                        Ok(_) => (),
                        Err(e) => {
                            eprintln!("[{}] Failed to backup.",job.name());
                            return Err(e);
                        }
                    }
                } else {
                    bail!("No job named '{}' found!",jobname);
                }
            } else {
                let mut run = 0;
                let mut failed = 0;
                for job in jobs.values_mut() {
                    match job.backup() {
                        Ok(_) => (),
                        Err(e) => {
                            failed += 1;
                            eprintln!("[{}]\tFailed to backup. {}",job.name(),e);
                        },
                    }
                    run += 1;
                }
                println!("Backup run finished. {}/{} jobs failed.",failed, run);
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
                        bail!("No job named '{}' found!",target_name);
                    }
                    None => {
                        bail!("Dry run flag requires a job name!");
                    }
                }
            }
            // println!("Backup starting time is {}",defaults.backup_start_time);
            for (_,job) in jobs.iter_mut() {
                match job.update_last_run() {
                    Ok(v) => {
                        let next_run = job.next_run()?;
                        println!(
                        "[{}]\tJob ok, found snapshots, last backup {}, next backup would be at {}",
                        job.name(),
                        job.last_run().expect("Expected at least one snapshot"),
                        next_run,
                    );
                    },
                    Err(e) => {
                        if e == CommandError::NotInitialized {
                            println!("[{}]\tRepo not initialized?",job.name());
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
        Commands::Daemon { } => {
            // update last_run for each job
            if jobs.is_empty() {
                bail!("No backup jobs configured!");
            }
            let mut jobs: Vec<_> = jobs.into_values().map(|mut v|{v.snapshots(Some(1)); v}).collect();

            loop {
                jobs.sort_unstable_by(|a,b|a.next_run().unwrap().cmp(&b.next_run().unwrap()));
                    
                while let Some(mut job) = jobs.pop() {
                    let now = time::OffsetDateTime::now_local()
                    .into_diagnostic()?;
                    let sleep_time = job.next_run()? - now;
                    if sleep_time.is_positive() {
                        std::thread::sleep(sleep_time.try_into().into_diagnostic()?);
                    }
                    match job.backup() {
                        Ok(s) => (),
                        Err(e) => {
                            eprintln!("[{}]\tFailed to backup.",job.name());
                            return Err(e);
                        }
                    }

                    jobs.push(job);
                }
            }
        },
    }

    Ok(())
}

fn read_config() -> Result<Conf> {
    let file = File::open("config.toml").into_diagnostic()?;
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
