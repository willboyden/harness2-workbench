# ADR 0009: Offline install bundles

- **Status:** Accepted. Contract addendum v1.5 ([ARCHITECTURE.md](../ARCHITECTURE.md) §16), additive.
- **Applies to:** `h2w-core` (`bundle.rs`), `h2w-daemon` (`bundles.rs`, the `bundle` command line).

## Context

Installing harness2 and a benchmark downloads from many hosts: GitHub (git and its API), the npm
registry and `pkg.pr.new` (OpenCode's lockfile), PyPI, and Hugging Face for JobBench. The egress proxy
of container isolation ([ADR 0006](0006-isolation-modes.md)) narrows that to a list, but some
deployments must not let the Workbench host or its jobs reach the internet at all, or want those
downloads done once, on a machine of their choosing, and kept. A LAB installation is large (about
7.8 GB: OpenCode with its `node_modules`, Bun, the LAB checkout with its Python environment and task
documents), and everything after setup (preview, run, judge, report) needs only the model endpoint.

## Decision

Three administrator commands on the host, `h2workbench bundle export | verify | import`:

- **Format.** A gzip-compressed tar: the manifest (`h2w-bundle.json`) first, then the runner image as
  the container runtime's `save` wrote it (only with `--with-runner-image`), then the managed
  installation under `harness2/`. The manifest records the format version, the exporting Workbench,
  the harness2 commit and version, the Python version and the interpreter directory its virtual
  environment needs, the CPU architecture, the installed benchmarks (pinned commit, data present),
  the file and byte counts, and the runner image's reference and id.
- **Integrity.** Export writes `FILE.sha256` (`sha256sum` format) and prints the same line. Import
  **requires `--sha256`**: the value must reach the importing administrator through a channel they
  trust; the sidecar next to the file proves nothing about the file.
- **Safety on read.** Every entry is checked before it becomes a path: relative, no `.`/`..`
  components; only the manifest, the image and `harness2/...`; only files, directories and symbolic
  links (no hard links, devices, FIFOs); no path below a symbolic link; relative link targets stay in
  the tree; absolute targets only for a virtual environment's interpreter (`.venv/bin/python*`); no
  duplicates; setuid, setgid and sticky bits dropped; the counts must match the manifest.
- **Compatibility.** Format version; CPU architecture (Bun and compiled wheels are native); the
  harness2 commit must be the tested one unless `paths.allow_untested_harness2_rev`; the interpreter the
  environment needs must exist where jobs run (on the host in host isolation; in the runner image in
  container isolation, asked of a throw-away container without network, or brought by the bundle).
- **Import.** Managed installations only; refused while any job is queued or running (a queued job
  could start between the two renames); a free-space check; unpacked into a private directory next
  to `paths.harness2_home`; the runner image loaded and its id checked (container isolation); then
  two renames: the current installation to `<home>.previous` (an older one is removed), the new one
  into place, the first undone if the second fails. Export and import are audited
  (`bundle.export`, `bundle.import`, actor `cli`).
- **Command line only.** Importing code is the most privileged thing the Workbench can do, so it is not
  a REST operation or an MCP tool: no token, session or agent can reach it.

## Consequences

Good: a Workbench can be set up with no egress at all, its jobs on `network = "none"`; the downloads
happen once and can be reviewed or scanned before import; the same bundle gives the same tree on
every host; the previous installation stays one rename away.

Bad, and how it is managed:

- **Size.** Gigabytes per benchmark; export and import take minutes. The commands report sizes and
  check free space first.
- **Ties.** A bundle fits one architecture, one harness2 commit and one interpreter location;
  `verify` names each mismatch before anything is changed, and `--with-runner-image` carries the
  exact image a container-isolation bundle was made with.
- **Trust.** A matching SHA-256 proves the bundle is the one exported, not that its contents are
  benign: it holds the same code an install would have downloaded. The documentation says so.
- **Scope.** Experiments are not included; they have their own backup procedure.

## Alternatives considered

- **Bake the installation into the runner image.** Rejected: the checkout is mounted so the image
  stays generic and a harness2 bump never needs a rebuild (ADR 0006); an 8 GB image would still be
  built with network access.
- **Package mirrors or a caching proxy** (npm, PyPI, git). Rejected for now: many hosts and protocols
  (git over HTTPS, tarball redirects, `pkg.pr.new`), each mirror to run and secure, and still egress
  per host.
- **Signed bundles** (minisign, Sigstore). Deferred: a key-management story for a property the
  out-of-band SHA-256 already gives one organisation.
- **A REST job or a web upload.** Rejected for now, for the reason under **Command line only**.

## Evidence in the repository

- `crates/h2w-core/src/bundle.rs`: format, checks and tests (round trip with modes and links, the
  runner image, counts against the manifest, fifteen hostile archives, manifest first, compatibility).
- `crates/h2w-daemon/src/bundles.rs`: the commands; `cli.rs`: their arguments (`--sha256` required on
  import).
- [ADMIN-GUIDE.md §3.5](../ADMIN-GUIDE.md#35-offline-installation-bundles): the procedure and what
  was verified.

## Revisit when

Bundles travel between organisations (signing), an administrator needs the import in the web UI, or
harness2 ships an offline installation of its own.
