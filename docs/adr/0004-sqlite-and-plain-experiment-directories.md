# ADR 0004: SQLite for state, files for logs, plain harness2 directories for experiments

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §7, §10).
- **Applies to:** `h2w-daemon` (`db/`, `jobs/logs.rs`, `expdirs.rs`), `h2w-core` (`results`).

## Context

The server must remember who may sign in, which jobs ran and how they ended, which experiments exist
and which model profile each is pinned to, the model profiles and secrets, and a hash-chained audit
log. It also produces large, append-only job logs, and it fronts results that harness2 itself
writes as thousands of files. It is a single-node tool, run by one team on one machine, often
started and stopped by hand.

## Decision

- **SQLite**, bundled with the binary (`rusqlite`), in WAL mode, for tokens, sessions, experiments,
  profiles, secrets, splits, jobs and the audit chain. One dedicated thread owns the connection and
  async code sends it closures, so statements are serialised and the database never blocks the async
  runtime. Schema changes are numbered migrations applied at start (`PRAGMA user_version`); a database
  from a newer build is refused. The file is created mode 0600 in a 0700 directory, with `foreign_keys`
  and `secure_delete` on and a 10-second busy timeout, so the command line (`token`, `audit verify`) can
  use the same file as a running server.
- **Files for logs.** Each job's log is a file of JSON lines (`<data>/logs/jobs/<id>.log`, 0600),
  with a bounded in-memory ring buffer for live streams. Retention deletes files by age; the job row
  stays.
- **Experiments remain plain harness2 directories** under `paths.experiments_dir`, one
  `HARNESS2_RUNS_DIR` each. The database holds only metadata about them (kind, description, tags, the
  pinned profile, who created them). `POST /experiments` can adopt a directory that already exists.

## Consequences

Good:

- **Zero operations.** No database server to install, secure, upgrade or back up separately; one
  portable file plus directories. Online backup is the SQLite backup API.
- harness2's own tools keep working on the same directories; the Workbench can be removed without
  converting data, and its database rebuilt for existing experiments by adopting them.
- Logs do not bloat or lock the database.

Bad, and how it is managed:

- **Single node, no high availability, one writer.** Fine for an administration tool whose write rate
  is a few requests and log lines per second; not a fit for a multi-node scheduler (a stated non-goal).
- **The database is the secret store, in plain text** ([THREAT-MODEL.md](../THREAT-MODEL.md)). File modes and an
  environment-variable alternative (`H2W_SECRET_<NAME>`) are the only protection; backups inherit the
  exposure.
- **Metadata and results can disagree** if someone edits directories by hand: an experiment whose
  directory is missing answers `the directory of experiment X is missing`.
- **No downgrade path** for the database: restore a backup to go back.
- **Retention is by age, not by size**; a runaway job can still fill a disk.

## Alternatives considered

- **PostgreSQL** (rejected in the original decision table): an operations burden (a second service,
  credentials, upgrades, backups) for a single-node tool, and an extra container in the reference
  deployment, for concurrency the workload does not need.
- **Storing logs in the database**: not recorded as evaluated; the decision above keeps the database
  small and the logs greppable instead.

## Evidence in the repository

`crates/h2w-daemon/src/db/mod.rs` (migrations, pragmas, modes), `db/audit.rs`, `jobs/logs.rs`,
`expdirs.rs`; tests `secrets.rs::state_files_and_directories_are_private`, `cli.rs`,
`audit.rs`. A restored online backup of a live database was opened and verified for this
documentation ([ADMIN-GUIDE.md](../ADMIN-GUIDE.md#12-data-backups-and-upgrades)).

## Revisit when

more than one server must share state, the write rate grows beyond what one writer thread can
serve, or secrets must be encrypted at rest with keys managed outside the database file.
