use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use clap::{Parser, Subcommand};
use config::Global;
use miette::{bail, Context, IntoDiagnostic, Result};
use serde::Deserialize;
use serde_with::serde_as;
use serde_with::DurationSeconds;

mod config;
mod job;

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
}

// /// Turn debugging information on
// #[arg(short, long)]
// test: bool,

// /// Run specific job by name
// #[arg(short, long)]
// job: Option<String>,

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = read_config().wrap_err("Reading configuration")?;
    match &cli.command {
        Commands::Run { job } => {
            unimplemented!()
        }
        Commands::Test {} => {
            check_restic(&config)?;
            let mut failed = 0;
            for job in &config.jobs {
                match job.snapshots(&config) {
                    Ok(v) => println!("Check for Job '{:?}' ok, found {} snapshots",job.name,v.len()),
                    Err(e) => {
                        eprintln!("Check for Job '{}' failed: {:?}",job.name,e);
                        failed += 1;
                    },
                }
            }
            if failed > 0 {
                eprintln!("Failed test for {} jobs",failed);
            } else {
                println!("Test successfull");
            }
        }
    }

    Ok(())
}

fn read_config() -> Result<Global> {
    let file = File::open("config.toml").into_diagnostic()?;
    let mut reader = BufReader::new(file);
    let mut cfg = String::new();
    reader.read_to_string(&mut cfg).into_diagnostic()?;

    let config: Global = toml::from_str(&cfg).into_diagnostic()?;
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