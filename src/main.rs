use std::{
    fs::File,
    io::{BufReader, Read},
    process::Command, collections::HashMap,
};

use clap::{Parser, Subcommand};
use config::{Global, Conf};
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
    Test {},
    /// Run all backup jobs once
    Run {
        /// Run specific job by name
        #[arg(short, long)]
        job: Option<String>,
        /// Abort on first error, stops any further jobs
        #[arg(short, long, default_value_t = false)]
        abort_on_error: bool,
        /// Force backups for all jobs, do not respect specified job intervals
        #[arg(short, long, default_value_t = false)]
        force: bool,
    },
    /// Daemonize and run backups in specified intervals
    Daemon {
        #[arg(short, long)]
        job: String,
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

    let (defaults, mut jobs) = config.split();

    match &cli.command {
        Commands::Run { job, abort_on_error, force } => {
            // maybe unify this here, but would require creating an ad-hoc iterator of
            // one element
            if let Some(jobname) = job {
                if let Some(job) = jobs.get_mut(jobname) {
                    println!("Job '{}' starting backup",job.name());
                    match job.backup() {
                        Ok(_) => println!("Job '{}' backup successfull.",job.name()),
                        Err(e) => {
                            eprintln!("Job '{}': Failed to backup.",job.name());
                            return Err(e);
                        }
                    }
                } else {
                    bail!("No job name '{}' found!",jobname);
                }
            } else {
                let mut run = 0;
                let mut failed = 0;
                for job in jobs.values_mut() {
                    println!("Job '{}' starting backup",job.name());
                    match job.backup() {
                        Ok(_) => println!("Job '{}' backup successfull.",job.name()),
                        Err(e) => {
                            failed += 1;
                            eprintln!("Job '{}': Failed to backup. {}",job.name(),e);
                        },
                    }
                    run += 1;
                }
                println!("Backup run. {} of {} jobs failed.",failed,run);
            }            
        }
        Commands::Test {} => {
            let mut failed = 0;
            for (_,job) in jobs.iter_mut() {
                match job.snapshots() {
                    Ok(v) => println!(
                        "[{}] Job ok, found {} snapshots",
                        job.name(),
                        v.len()
                    ),
                    Err(e) => {
                        if e == CommandError::NotInitialized {
                            println!("[{}] Repo not initialized?",job.name());
                        } else {
                            eprintln!("[{}] check failed: {}", job.name(), e);
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
        Commands::Daemon { job } => todo!(),
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
