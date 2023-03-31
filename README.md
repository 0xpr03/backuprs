# Restic backup manager

Perform multiple backup jobs for different repositories and users towards a restic-server.

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

### Restic

To run backups, [restic](https://restic.readthedocs.io/en/stable/020_installation.html) itself is required. You can specify the binary path in the configuration.

### Postgres Dumbs

If you want to perform postgres backups, you can install the postgres dumb tool `pg_dump`. To use it, you have to specify the path towards the binary in your configuration. On Windows you may find it inside `C:\Program Files\PostgreSQL\XX\bin`. On Linux you can install the `postgresql-client` package.

### MySQL Dumbs

For MySQL it is the same story as for Postgres: You need to have the database dumb binary installed and a path in your configuration.
