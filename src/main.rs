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
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Test config
    Test {},
    Run {
        /// Run specific job by name
        #[arg(short, long)]
        job: Option<String>,
    },
    Debug {
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
    config.global.check()?;
    check_restic(&config.global)?;
    let (defaults, mut jobs) = config.split();
    match &cli.command {
        Commands::Run { job } => {
            for (name,job) in jobs {
                // job.
            }
        }
        Commands::Test {} => {
            let mut failed = 0;
            for (_,job) in jobs.iter_mut() {
                match job.snapshots() {
                    Ok(v) => println!(
                        "Check for Job '{:?}' ok, found {} snapshots",
                        job.name(),
                        v.len()
                    ),
                    Err(e) => {
                        if e == CommandError::NotInitialized {
                            println!("Job '{}': Repo not initialized?",job.name());
                        } else {
                            eprintln!("Check for Job '{}' failed: {}", job.name(), e);
                        }
                        failed += 1;
                    }
                }
            }
            if failed > 0 {
                eprintln!("Failed test for {} jobs", failed);
            } else {
                println!("Test successfull");
            }
        }
        Commands::Debug { job } => {

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
