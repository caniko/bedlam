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
  "secretFiles": {"GITHUB_TOKEN": "/run/user/1000/agenix/github-token"}
}
```

Hosts take an optional `"priority"` (u8, default 0, e.g. 1 for workers): placement prefers the highest priority, then the most free budget, then `localHost`.

Every host needs `mutiny` and `amc` (the `amc admission exec` CLI) on its non-interactive `PATH`, a shared `pool.json`, and key-based SSH.

## Commands

- `mutiny host status --json` - `{host, eligible, freeBudgetMiB, memAvailableMiB, memFullPsi, onAc, reason?}`.
  Missing or duplicate readings, or no read-write `/dev/kvm`, make the host ineligible.
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
That unit queues for AMC admission via `amc admission exec --queue` (contract from `pool.json`, memory = VM RAM + 512 MiB, 5 min wait) and holds the
lease for the VM's lifetime, independent of the launching SSH session. Destroying the sandbox (or stopping the unit)
ends the VM and releases the lease.

## Guest

The guest is `docker.io/nixos/nix`, pinned by digest (Nix 2.35.4), with a 16 GiB root disk and
`experimental-features = nix-command flakes`. The repo is cloned to `/work/repo`; agents run toolchains with
`nix develop -c ...` via `exec` (cache.nixos.org only). No host directory is mounted, so untrusted guest code cannot poison host caches
(a leftover `sccacheDir` in an old `pool.json` is ignored).

Egress uses microsandbox's default policy (the `Public` profile): public internet only, private/LAN ranges and the host
are denied. Mutiny sets no network policy of its own.

Secrets from `secretFiles` go through microsandbox's secret proxy: the guest sees a placeholder, and the real value is
substituted only in the `Authorization` header of requests to `github.com` / `api.github.com`, and only after the proxy
has verified the server's TLS identity. Git in the guest authenticates through a global credential helper that sends
`x-access-token:$GITHUB_TOKEN` as Basic auth (GitHub's git smart-HTTP rejects `Bearer`); the proxy substitutes inside
the base64 credentials, so clones, fetches and pushes all work.

## Capacity and validation

A host's free budget is the AMC `admission status` of the user's ledger; if the shared-host broker socket
(`/run/amc-host/admission.sock`) exists it is the lower of that and `admission host-status`, and an unreadable
broker makes the host ineligible.

Sandbox names must be `mu-` prefixed with `[A-Za-z0-9_.-]`, for every verb and for `forkOf`; `repo` must be exactly
`owner/name`. SSH to pool hosts uses `StrictHostKeyChecking=yes`, so add each host key to `known_hosts` first.

## Security

The libkrun VMM runs as the unprivileged user inside the user's systemd manager, so a guest escape equals that
user's account. This is an accepted risk.

## Build

`microsandbox` is used with `default-features = false, features = ["local", "net"]`, so the build does not download the
runtime. At run time the host needs `msb` and `libkrunfw` (install via the microsandbox installer, or set
`MSB_PATH` / `MSB_LIBKRUNFW_PATH`) and `/dev/kvm`.

`cargo test` runs the placement, AMC-status parse and validation tests; `cargo test -- --ignored` also boots a real
guest (needs `/dev/kvm` and `MSB_PATH` / `MSB_LIBKRUNFW_PATH`). Licensed Apache-2.0.
