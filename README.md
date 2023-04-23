# Restic backup manager

Perform multiple [restic](https://restic.net/) backup jobs for different repositories and users.

```text
Usage: backuprs [OPTIONS] <COMMAND>

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

```text
Force run all or one backup job

Usage: backuprs run [OPTIONS]

Options:
  -j, --job <JOB>       Run specific job by name
  -a, --abort-on-error  Abort on first error, stops any further jobs
  -h, --help            Print help
```

```text
Test config or perform dry-runs

Usage: backuprs test [OPTIONS]

Options:
      --dry-run
          Dry run, do not perform backup, only print what would happen.
          
          Equals `restic backup --dry-run`. Requires job argument.

  -j, --job <JOB>
          Test specific job by name

  -h, --help
          Print help (see a summary with '-h')
```

## Features

- Multiple restic backups jobs with different configurations.
- Override any defaults per job.
- Supported backends currentls are
  - [Rest-Server](https://github.com/restic/rest-server) backends are currently supported
  - S3
  - SFTP with custom connection parameters.
- Timeframe where all backups are allowed to run.
- Interval for each backup job.
- Automic repository initialization.
- Pre- and Post-Backup commands.
- Mysql and PostgreSQL backup support.

## Installation

This section covers the installation.

### Building from source

- Install [rust](https://www.rust-lang.org/tools/install)
- Build the application via `cargo build --release`
- The final binary is inside `target/release/`. You can also run `cargo run --release` to invoke it.
- On Linux copy the binary to a secure location, and change the owners, such that it can't be modified by anyone other than root.

### Setup

If possible run backuprs in its own user and service unit, which you can lock down against external access.

The following paths have to accessible:
- `~/.cache` For restic
- `~/scratchspace` For database dumps or script runs that require an additional folder. You can change this path.

A full setup can look like this:
Create the user
`sudo useradd --system --create-home --shell /sbin/nologin backuprs`
Create the folder:
```sh
sudo mkdir /home/backuprs/bin # binaries
sudo mkdir /home/backuprs/.cache # restic
sudo mkdir /home/backuprs/scratchspace # DB dumps
sudo chmod 770 -R /home/backuprs
```
Install restic client:
`curl -L https://github.com/restic/restic/releases/download/v0.15.1/restic_0.15.1_linux_amd64.bz2 | bunzip2 > /home/backuprs/bin/restic`
Set permissions:
```sh
sudo chown root:backuprs /home/backuprs/bin/restic
sudo chmod 750 /home/backuprs/bin/restic
```
Copy backuprs to /home/backuprs/bin/backuprs
`sudo chmod 750 /home/backuprs/bin/backuprs`

Add the service unit:
Copy `backup.service` to `/etc/systemd/system/backup.service`
Reload systemd
`sudo systemctl daemon-reload`

Now copy the configuration `config.toml.example` to `config.toml` and secure it:
`sudo chmod 700 /home/backuprs/config.toml`

To start the service run 
```sh
sudo systemctl start backup.service
sudo systemctl enable backup.service
```

### Configuration

- Copy `config.toml.example` to `config.toml`. If you're on linux, you also have to guard the file against access through other users `chmod o= config.toml`.
- Adapt the configuration to your needs, see below for restic & database integration. You have to specify the path towards the restic binary.
- Test your configuration via `backuprs test`.

See below for more information of specific parts of the configuration.

A systemd service unit can be found in `backup.service`.

### Restic

To run backups, [restic](https://restic.readthedocs.io/en/stable/020_installation.html) itself is required. You can specify the binary path in the configuration.

### scratch_dir

The `scratch_dir` path should point towards a directory which can be used freely by backuprs when performing database backups. It is also handed towards user provided post/pre-commands. It should therefore not be readable by anyone other user, as it may contain your sensitive data.

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

Instead of using user/pasword for login, you can only specify the database name, and mysql expectes a [.my.cnf](https://dev.mysql.com/doc/refman/8.0/en/option-files.html) in the backuprs home folder, which contains the login data:

```toml
[mysqldump]
user="mysqluser"
password="secret"
```

A global backup user can be created via
```sql
CREATE USER 'backuprs'@'localhost' IDENTIFIED BY '<CHANGE ME>';
GRANT SELECT, SHOW VIEW, LOCK TABLES, RELOAD, REPLICATION CLIENT ON *.* TO 'backuprs'@'localhost';
FLUSH PRIVILEGES;
```

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
