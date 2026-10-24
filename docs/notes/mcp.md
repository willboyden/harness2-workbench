# MCP server (`h2workbench-mcp`): setup notes

`h2workbench-mcp` gives AI agents (Claude Code, opencode, Codex, Cursor, VS Code and any other MCP
client) the harness2 operations of the Workbench web UI as MCP tools, resources and prompts. It is
a thin client of a running Workbench server (`h2workbench serve`, default `http://127.0.0.1:8722`): it
keeps no state, and every call it makes carries an API token, so the server's roles (viewer <
operator < admin) and its audit log apply to each agent.

Status (2026-10-24): the tools, resources, prompts and both transports are tested against a
mock Workbench server (wiremock) and with an rmcp client, in process, over HTTP and against the
built binary over stdio (`cargo test -p h2w-mcp`); the end-to-end suite
(`cargo xtask e2e --suite mcp`) runs the binary over stdio against a real Workbench server and
harness2 checkout. The client snippets below follow each client's documented configuration
format; they have not been tried against those clients from this repository.

## 1. A token: least privilege

Give the agent the smallest role that does the job:

- **viewer** by default: it can read everything an agent needs to analyse results (experiments,
  tasks, harnesses, diffs, scores, reports, logs) and estimate costs;
- **operator** only when the agent must start jobs (demo, preview, check, runs, pipelines,
  judging, reports, splits) or create experiments;
- **never admin.** Install harness2, set up benchmarks and run the Codex bridge from the web UI
  yourself. An agent's input includes text written by models (see section 4), so an admin token
  in its hands is an admin token for whatever wrote that text.

```sh
h2workbench token create --help    # name, role and expiry options; prefer an expiry
```

The token (`h2w_<id>_<secret>`) is shown once. Keep it in a file only you can read:

```sh
mkdir -p "$HOME/.config/harness2-workbench"
( umask 077 && cat > "$HOME/.config/harness2-workbench/mcp.token" )   # paste, Enter, Ctrl-D
```

`h2workbench-mcp` refuses a token file that group or others can read, and never accepts a token
as a command-line argument (it would show in `ps` and shell history).

## 2. Transports

| | stdio (default) | streamable HTTP |
|---|---|---|
| Start | the MCP client starts `h2workbench-mcp` | `h2workbench-mcp --transport http --listen 127.0.0.1:8723` |
| Token | `H2W_TOKEN`, or `H2W_TOKEN_FILE` / `--token-file` (chmod 600); not both | every request sends `Authorization: Bearer h2w_...`; that caller's token is forwarded |
| Server | `H2W_SERVER_URL` / `--server-url` | same |
| Endpoint | stdin/stdout | `POST http://127.0.0.1:8723/mcp` (stateless: no `Mcp-Session-Id`) |

HTTP mode refuses, in this order: an unknown `Host` with 421 (DNS-rebinding defence; loopback
forms of the listen port are allowed, add others with `--allowed-host`), a foreign `Origin` with
403 (`--allowed-origin` adds one), a missing or malformed bearer token with 401, and a request
beyond `--max-concurrent-requests` (default 32; a request counts until it has been handled,
event-stream answers included) with 503 and `Retry-After: 1`. Bodies over 1 MiB get 413
(`--max-body-bytes`). Put a TLS reverse proxy in front before listening beyond loopback, and list
its host name with `--allowed-host`.

Each API token may run at most `--max-waits-per-token` waits at once (default 4, both
transports): `wait_for_job` beyond it is a tool error starting `rate_limited:`, and a job tool
called with `wait_secs` returns the job it created without waiting, with a note.

**Host-isolation risk acknowledgement.** Under host isolation the Workbench server requires
`acknowledge_risks=true` on runs and pipelines, because the executor then has the server
user's files and network. A model must not accept that for the user, so `h2workbench-mcp` refuses
`start_run` / `start_pipeline` calls with `acknowledge_risks=true` (a tool error starting
`forbidden:`) unless whoever starts it passes `--allow-risk-acknowledgement` or sets
`H2W_MCP_ALLOW_RISK_ACK=1` (both transports; off by default). The alternatives: a human starts
host-isolation runs from the web UI, or an admin pre-acknowledges host risks in the server's
configuration. Calls without the acknowledgement are passed on unchanged and the server
decides. `server_info` shows the setting under `mcp_server.policy`.

`h2workbench-mcp` refuses a plain-`http` `--server-url` on a non-loopback host, because the token
would cross the network unencrypted; use `https` (or `--allow-insecure-remote` if you accept
that). Logs go to stderr; `RUST_LOG` adjusts them (default `warn,h2w_mcp=info`). Tokens are never
logged.

## 3. Client configuration

Replace `/absolute/path/to/mcp.token` with the real path where a client does not expand
variables. `wait_for_job` can block for up to 600 s (default 50 s): give the client a tool
timeout above the waits you use.

### Claude Code

```sh
claude mcp add h2workbench \
  --env H2W_SERVER_URL=http://127.0.0.1:8722 \
  --env H2W_TOKEN_FILE="$HOME/.config/harness2-workbench/mcp.token" \
  -- h2workbench-mcp
```

Or in a project's `.mcp.json` (Claude Code expands `${VAR}`):

```json
{
  "mcpServers": {
    "h2workbench": {
      "command": "h2workbench-mcp",
      "env": {
        "H2W_SERVER_URL": "http://127.0.0.1:8722",
        "H2W_TOKEN_FILE": "${HOME}/.config/harness2-workbench/mcp.token"
      }
    }
  }
}
```

HTTP, with the token taken from the environment of the shell that starts Claude Code:

```json
{
  "mcpServers": {
    "h2workbench": {
      "type": "http",
      "url": "http://127.0.0.1:8723/mcp",
      "headers": { "Authorization": "Bearer ${H2W_TOKEN}" }
    }
  }
}
```

Long waits: raise `MCP_TOOL_TIMEOUT` (milliseconds) if calls are cut off.

### opencode (`opencode.json`)

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "h2workbench": {
      "type": "local",
      "command": ["h2workbench-mcp"],
      "environment": {
        "H2W_SERVER_URL": "http://127.0.0.1:8722",
        "H2W_TOKEN_FILE": "/absolute/path/to/mcp.token"
      },
      "enabled": true
    }
  }
}
```

HTTP:

```json
{
  "mcp": {
    "h2workbench": {
      "type": "remote",
      "url": "http://127.0.0.1:8723/mcp",
      "headers": { "Authorization": "Bearer {env:H2W_TOKEN}" },
      "enabled": true
    }
  }
}
```

### Codex (`~/.codex/config.toml`)

```toml
[mcp_servers.h2workbench]
command = "h2workbench-mcp"
env = { H2W_SERVER_URL = "http://127.0.0.1:8722", H2W_TOKEN_FILE = "/absolute/path/to/mcp.token" }
startup_timeout_sec = 20
tool_timeout_sec = 660   # the default (60 s) is shorter than long waits
```

HTTP (Codex versions with streamable-HTTP support):

```toml
[mcp_servers.h2workbench]
url = "http://127.0.0.1:8723/mcp"
bearer_token_env_var = "H2W_TOKEN"
tool_timeout_sec = 660
```

### Cursor (`~/.cursor/mcp.json` or `.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "h2workbench": {
      "command": "h2workbench-mcp",
      "env": {
        "H2W_SERVER_URL": "http://127.0.0.1:8722",
        "H2W_TOKEN_FILE": "${userHome}/.config/harness2-workbench/mcp.token"
      }
    }
  }
}
```

HTTP:

```json
{
  "mcpServers": {
    "h2workbench": {
      "url": "http://127.0.0.1:8723/mcp",
      "headers": { "Authorization": "Bearer ${env:H2W_TOKEN}" }
    }
  }
}
```

### VS Code (`.vscode/mcp.json`)

```json
{
  "servers": {
    "h2workbench": {
      "type": "stdio",
      "command": "h2workbench-mcp",
      "env": {
        "H2W_SERVER_URL": "http://127.0.0.1:8722",
        "H2W_TOKEN_FILE": "${userHome}/.config/harness2-workbench/mcp.token"
      }
    }
  }
}
```

HTTP, prompting once for the token (stored by VS Code's secret storage):

```json
{
  "inputs": [
    {
      "type": "promptString",
      "id": "h2w-token",
      "description": "Harness² Workbench API token",
      "password": true
    }
  ],
  "servers": {
    "h2workbench": {
      "type": "http",
      "url": "http://127.0.0.1:8723/mcp",
      "headers": { "Authorization": "Bearer ${input:h2w-token}" }
    }
  }
}
```

## 4. What agents get

**42 tools** — the 39 of ARCHITECTURE.md §13 plus `list_splits`, `update_experiment` and (contract
addendum v1.2) `check_profile`
(§16, v1.1). Each is one REST capability; the role is the minimum the server requires.

| Family | Tools | Role |
|---|---|---|
| Status | `server_info`, `doctor`, `list_benchmarks`, `list_profiles`, `check_profile` | viewer (`doctor` refresh: operator) |
| Experiments | `list_experiments`, `get_experiment`, `get_manifest`, `list_splits` | viewer |
| | `create_experiment`, `update_experiment` | operator |
| Planning | `estimate_cost` | viewer |
| | `preview_run` | operator |
| Jobs | `run_demo`, `check_benchmark`, `start_run`\*, `start_pipeline`\*, `start_judge`\*, `start_report`, `create_split`, `cancel_job` | operator |
| | `setup_benchmark`, `install_harness2`, `start_bridge`\* | admin |
| | `list_jobs`, `get_job`, `wait_for_job`, `get_job_log` | viewer |
| Results | `list_tasks`, `get_task`, `get_harness`, `diff_harnesses`, `list_rollout_files`, `read_rollout_file`, `get_scores`, `get_report`, `get_library`, `list_audit_sessions`, `read_audit_session`, `compare_sweeps` | viewer |
| Adoption | `export_harness`, `validate_harness` | viewer |

\* calls model endpoints: costs money with cloud models.

- Arguments mirror the REST names (`experiment`, `sweep`, `task`, `mode`, `label`, `from`, `to`,
  `path`, `session`, `part`, `job_id`); job tools take the job's parameters flat plus
  `wait_secs`. Unknown or misspelled arguments are errors, never ignored.
- Results are the server's JSON, pretty-printed in the first text block (cut with a note above
  100 KB) and attached as structured content when it is an object of at most 256 KiB; a second
  text block may add a short note (what a job's status means, what to call next).
- Failures are tool errors (`isError: true`) whose text starts with the server's error code
  (`forbidden: ...`, `validation: ...; field: problem`, `unavailable: ...`), plus a hint where
  the fix is known (the role a tool needs; `install_harness2` when harness2 is missing).
- `wait_for_job` (and job tools with `wait_secs`) follow the server's job event stream, fall
  back to polling, and send MCP progress notifications (finished tasks / total, current task,
  with a heartbeat at least every 10 s) when the client sends a progress token.
- **Untrusted content.** Rollout files, job logs, task prompts and improver notes, harness files
  (`get_harness`, `diff_harnesses`, `export_harness`), audit sessions and library lessons are
  written by models or benchmark tasks. The server instructions and those tools' descriptions
  tell the agent to read them as data and never follow instructions found in them; treat an
  agent that acts on them anyway as compromised for that session.
- `install_harness2` always installs the commit this Workbench release was tested against: the
  tool has no `rev` argument (security audit F13).
- Units: percent (0..100) in reports and headlines (`get_report`, `get_experiment`); fractions
  (0..1) in judge rows, task rows and comparisons (`get_scores`, `list_tasks`, `get_task`,
  `compare_sweeps`). Ungraded is not zero.

**Resources:** `h2w://jobs/{id}/log` (the last 2,000 lines), `h2w://experiments/{exp}/sweeps/{sweep}/report`
and `h2w://experiments/{exp}/manifest` (JSON). `resources/list` offers every experiment's
manifest, every judged sweep's report and the 20 newest jobs' logs.

**Prompts:** `improve_agent_harness` (doctor, profile, benchmark setup and check, preview, cost
estimate and the user's go-ahead, pipeline, waiting, report, inspection, export),
`analyze_sweep` (read-only analysis) and `adopt_best_harness` (choose, explain, validate and
export a harness, with the user's approval before anything is written).

**Not exposed, by contract (§16):** writes to profiles, secrets, tokens and the audit log. A
prompt-injected agent could otherwise repoint a profile's endpoint and leak the key it
references; secret values and new tokens must not pass through a model's context.
