# Restic backup manager

Perform multiple backup jobs for different repositories and users towards a restic-server.

```text
Usage: backuprs.exe [OPTIONS] <COMMAND>

Commands:
  test    Test config or perform dry-runs
  run     Force run all or one backup job
  daemon  Daemonize and run backups in specified intervals
  help    Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose  Verbose output
  -h, --help     Print help
  -V, --version  Print version
```

## Installation

This section covers the installation.

### Building from source

- Install [rust](https://www.rust-lang.org/tools/install)
- Build the application via `cargo build --release`
- The final binary is inside `target/release/`. You can also run `cargo run --release` to invoke it.

### Configuration

- Copy `config.toml.example` to `config.toml`. If you're on linux, you also have to guard the file against access through other users `chmod o= config.toml`.
- Adapt the configuration to your needs, see below for restic & database integration. You have to specify the path towards the restic binary.
- Test your configuration via `backuprs test`.

See below for more information of specific parts of the configuration.

### Restic

To run backups, [restic](https://restic.readthedocs.io/en/stable/020_installation.html) itself is required. You can specify the binary path in the configuration.

### Postgres Dumbs

If you want to perform postgres backups, you can install the postgres dumb tool `pg_dump`. To use it, you have to specify the path towards the binary in your configuration. On Windows you may find it inside `C:\Program Files\PostgreSQL\XX\bin`. On Linux you can install the `postgresql-client` package.

### MySQL Dumbs

For MySQL it is the same story as for Postgres: You need to have the database dumb binary installed and a path in your configuration.

## Configuration

Backups are run in the specified intervalls and time frame, the time frame has priority over the interval.

### Pre and Post commands

User supplied commands can be invoked via pre-/post-backup commands.
The following environment variables are passed:
- `BACKUPRS_JOB_NAME` The current jobs name
- `BACKUPRS_TARGETS` Paths for backup 
- `BACKUPRS_TEMP_FOLDER` a temporary folder that is deleted when the backup is finished (on failure and success). This folder is also used for database backups.
- `BACKUPRS_SUCCESS` whether the backup succeeded in running, this is only relevant for post commands with `post_command_on_failure` set. And always set true for pre commands.
Note that the full environment of backups is passed to the commands.
If `post_command_on_failure` is set, commands are run even when the backup fails.