# Mutiny

Open, self-hosted answer to [boat.dev](https://boat.dev): per-second microVM sandboxes for coding agents,
on your own hardware. The name is the crew taking back the boat.

Mutiny boots [microsandbox](https://github.com/superradcompany/microsandbox) (libkrun) VMs on whichever host of a
small pool has capacity according to [AMC](https://github.com/caniko/amc) admission, and exposes them to agents
through a local stdio MCP server.

## pool.json

Rendered by canix at `$XDG_CONFIG_HOME/mutiny/pool.json` (or pass `--pool <path>`). Weak hosts are excluded
upstream; Mutiny trusts `hosts`.

```json
{
  "user": "can",
  "localHost": "atlas",
  "contract": "sandbox",
  "hosts": [{"name": "atlas", "address": "192.168.178.2", "sshPort": 22}],
  "sccacheDir": "/home/can/.cache/sccache",
  "secretFiles": {"GITHUB_TOKEN": "/run/user/1000/agenix/github-token"}
}
```

Every host needs `mutiny` and `amc` on its non-interactive `PATH`, a shared `pool.json`, and key-based SSH.

## Commands

- `mutiny host status --json` - `{host, eligible, freeBudgetMiB, memAvailableMiB, memFullPsi, onAc, reason?}`.
  Missing or duplicate readings make the host ineligible.
- `mutiny host run --spec '{"repo":"o/r","ref":"main","cpus":2,"memoryMiB":2048}'` - start a sandbox here, print `{"id":"<host>:<name>"}`.
- `mutiny host exec|read|write|fork|snapshot|destroy|list` - operate on local sandboxes by name (`write` takes content on stdin).
- `mutiny mcp` - stdio MCP server.

## MCP tools

`pool_status`, `sandbox_create {repo, ref, cpus?, memoryMiB?}`, `exec {id, command}`, `read_file {id, path}`,
`write_file {id, path, content}`, `fork {id}`, `snapshot {id}`, `destroy {id}`, `list`.

`sandbox_create` queries all hosts in parallel (12 s cap), drops ineligible or unreachable ones, picks the most
free AMC budget (ties go to the local host) and runs `mutiny host run` there. Other tools route by the id's host prefix.

## AMC lease semantics

`host run` re-execs itself in a transient `systemd-run --user --unit mutiny-<name>` and returns once the VM is ready.
That unit queues for AMC admission (contract from `pool.json`, memory = VM RAM + 512 MiB, 5 min wait) and holds the
lease for the VM's lifetime, independent of the launching SSH session. Destroying the sandbox (or stopping the unit)
ends the VM and releases the lease.

Secrets from `secretFiles` go through microsandbox's secret proxy: the guest sees a placeholder, and the real value is
substituted only on requests to `github.com` / `api.github.com`. The `sccacheDir` is bind-mounted at the same path
with `RUSTC_WRAPPER=sccache` and `SCCACHE_DIR` set.

## Build

`microsandbox` is used with `default-features = false, features = ["local", "net"]`, so the build does not download the
runtime. At run time the host needs `msb` and `libkrunfw` (install via the microsandbox installer, or set
`MSB_PATH` / `MSB_LIBKRUNFW_PATH`) and `/dev/kvm`.

`cargo test` runs the placement and AMC-status parse tests. Licensed Apache-2.0.
