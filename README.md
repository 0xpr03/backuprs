# Restic backup manager

Perform multiple [restic](https://restic.net/) backup jobs for different repositories and users towards a restic-server.

```text
Usage: backuprs.exe [OPTIONS] <COMMAND>

Commands:
  test    Test config or perform dry-runs
  run     Force run all or one backup job
  daemon  Daemonize and run backups in specified intervals
  help    Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose      Verbose output
  -n, --no-progress  Disable progress output for backups
  -h, --help         Print help
  -V, --version      Print version
```

## Features

- Multiple restic backups jobs with different configurations
- **only** [Rest-Server](https://github.com/restic/rest-server) backends are currently supported
- Timeframe where all backups are allowed to run
- Interval for each backup job
- Pre- and Post-Backup commands
- Mysql and PostgreSQL backup support

## Installation

This section covers the installation.

### Building from source

- Install [rust](https://www.rust-lang.org/tools/install)
- Build the application via `cargo build --release`
- The final binary is inside `target/release/`. You can also run `cargo run --release` to invoke it.
- On Linux copy the binary to a secure location, and change the owners, such that it can't be modified by anyone other than root.

### Configuration

- Copy `config.toml.example` to `config.toml`. If you're on linux, you also have to guard the file against access through other users `chmod o= config.toml`.
- Adapt the configuration to your needs, see below for restic & database integration. You have to specify the path towards the restic binary.
- Test your configuration via `backuprs test`.

See below for more information of specific parts of the configuration.

### Restic

To run backups, [restic](https://restic.readthedocs.io/en/stable/020_installation.html) itself is required. You can specify the binary path in the configuration.

### scratch_dir

The `scratch_dir` path should point towards a directory which can be used freely by backuprs when performing database backups. It is also handed towards user provided post/pre-commands run.

The technical background is to use a static path for restic when backing up databases. Using temporary, unique folders (for example in `/tmp`), would prevent incremental backups of database dumps, resulting in a full new file per backup run.

### Postgres Backups

If you want to perform postgres backups, you can install the postgres dump tool `pg_dump`. To use it, you have to specify the path towards the binary in your configuration. On Windows you may find it inside `C:\Program Files\PostgreSQL\XX\bin`. On Linux you can install the `postgresql-client` package.

For authentication you can either tell backuprs to switch the user (requires `sudo`), or use a password:
```toml
postgres_db = { database = "database", change_user = false, user = "user", password = "password" }
```
You can leave options blank which you don't want to use, except for `database`.

### MySQL Backups

For MySQL it is the same story as for Postgres: You need to have the database dump binary installed and the path in your configuration.

Instead of using user/pasword for login, you can only specify the database name, and mysql expectes a [.my.cnf](https://dev.mysql.com/doc/refman/8.0/en/option-files.html) in the backuprs home folder, which contains the login data.

## Configuration

Backups are run in specified intervalls and time frame, the time frame has priority over the interval.

### Pre and Post commands

User supplied commands can be invoked via pre-/post-backup commands.
The following environment variables are passed:
- `BACKUPRS_JOB_NAME` The current jobs name
- `BACKUPRS_TARGETS` Paths for backup, delimited by `;`
- `BACKUPRS_EXCLUDES` Exclude paths for backup, delimited by `;`
- `BACKUPRS_TEMP_FOLDER` path to a temporary folder that is deleted when the backup is finished (on failure and success). This folder is also used for database backups.
- `BACKUPRS_SUCCESS` whether the backup succeeded in running, this is only relevant for post commands with `post_command_on_failure` set. And always set true for pre commands.
Note that the full environment of backups is passed to the commands.
If `post_command_on_failure` is set, commands are run even when the backup fails.