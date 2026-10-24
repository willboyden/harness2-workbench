# ADR 0006: Two isolation modes, `host` and `container`

- **Status:** Accepted. Part of the v1.0 contract ([ARCHITECTURE.md](../ARCHITECTURE.md) §2, §9), extended by addenda v1.2 (job egress, container `HOME`).
- **Applies to:** `h2s-core` (`isolation.rs`, `env.rs`), `h2s-daemon` (`jobs/`, `egress.rs`), `deploy/`.

## Context

harness2's executors are language-model agents that run shell commands and scripts. Upstream runs
them **unsandboxed and with network access**; Codex is started with its own sandbox disabled, and
upstream's guide tells operators to restrict the network at the host level. A server that lets several
people start such runs, and holds provider keys for them, cannot treat that as someone else's concern.
At the same time, most people trying harness2 on their own workstation want the upstream behaviour,
and some pieces (WorkBuddy-Bench, which drives Docker itself; the Codex bridge, which listens for
containers) do not fit inside a sandbox.

## Decision

Offer two modes, chosen by `isolation.mode`:

- **`host`** (default): jobs run as the server's user, exactly as upstream does. Because that is
  powerful, the server logs a warning at start-up and **refuses `run` and `pipeline` jobs that do not
  carry `acknowledge_risks: true`**, unless the administrator pre-acknowledges
  (`isolation.preacknowledge_host_risks`). The acknowledgement and every job are audited.
- **`container`**: each job runs in a throw-away container from a **pinned** runner image
  (`docker run` or `podman run`) with fixed hardening that no request can change: `--rm`,
  `--cap-drop ALL`, `--security-opt no-new-privileges`, pid, CPU and memory limits, the configured
  network (`none`, or an internal network behind `h2studio egress-proxy`), three bind mounts at fixed
  points (the experiment read-write, the harness2 checkout read-only except during install and setup,
  the job's generated files read-only), a job label verified before any stop, and no `host` network.
  The **egress proxy** is a hostname allowlist with address checks, used with an internal network so
  that it is the only way out.

Container mode is refused for WorkBuddy-Bench and the Codex bridge (`409`).

## Consequences

Good:

- The risk of host mode is explicit, acknowledged and recorded; the safer mode is available without a
  separate product.
- The same argv drives both modes ([ADR 0001](0001-wrap-the-harness2-cli.md)); container mode wraps
  rather than rewrites.
- In container mode a job cannot see the server's home directory, its database or other experiments,
  and its network can be limited to named hosts.

Bad, and how it is managed:

- **Container mode is hardening, not a cure.** The job still holds its profile's keys, still runs
  model-written code, and can still reach whatever the allowlist opens ([THREAT-MODEL.md](../THREAT-MODEL.md)).
- **A second code path** to keep correct. Host mode is exercised end to end against a real harness2;
  container mode is covered by argv tests and by the maintainers' live runs on rootless Docker, not by
  rootful Docker or Podman.
- **User mapping is subtle.** On a rootless runtime only container uid 0 can write the bind-mounted
  experiment (and it holds no capabilities); on a rootful one the server's own uid is passed. A runner
  image must give that user a writable `HOME` (the shipped one sets `HOME=/h2s/home`).
- **harness2 must be installed in the same mode** it runs in: a virtual environment created on the host
  points at the host's Python and fails inside the image (`exec ... failed: No such file or directory`).
- **The proxy is a hostname filter**: no TLS inspection, no client authentication; it governs only what
  is forced through it, and the Studio does not verify that the configured job network is internal. The same list serves install and evaluation, which upstream wants kept
  apart (the Studio documents the workflow; it does not automate it).
- **More moving parts**: images, networks, the proxy container, a runtime socket for the server.
- **Two ways of "contained"**: the compose deployment contains the Studio and its host-mode jobs
  together; per-job containers separate jobs from the server. They protect different things.

## Alternatives considered

- **Host mode only** (rejected in the original decision table): upstream's behaviour offers no
  containment, and organisations that need it would have to build it around the Studio.
- **Container mode only**: not recorded as evaluated; the constraints above (WorkBuddy, the bridge,
  machines without a container runtime) and the wish to match upstream on a workstation point against it.

## Evidence in the repository

`crates/h2s-core/src/isolation.rs` (`run_argv` and its refusals), `crates/h2s-daemon/src/jobs/`
(`container.rs`, `core_backend.rs`, `mod.rs`), `egress.rs`, `deploy/docker/`, `deploy/compose.yaml`.
Tests: `jobs.rs::workbuddy_and_the_bridge_refuse_container_isolation`, `preacknowledged_host_risks_need_no_flag`,
`core_real.rs::container_plans_use_fixed_hardening_and_numeric_users`, `egress.rs`. Verified live by the
maintainers on rootless Docker (install through the proxy, the demo as a container job, cancel by label);
not on rootful Docker or Podman ([ADMIN-GUIDE.md](../ADMIN-GUIDE.md#8-container-isolation-step-by-step)).

## Revisit when

a stronger sandbox (a user-space kernel, a micro-VM) becomes practical for the executors, when
WorkBuddy-Bench can run its containers under the Studio's control, or when the proxy needs to inspect or
authenticate traffic.
