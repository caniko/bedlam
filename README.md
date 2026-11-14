# Bedlam

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.github/workflows/ci.yaml) [![Nix](https://img.shields.io/badge/Nix-managed-5277c3)](flake.nix) [![crates.io](https://img.shields.io/badge/crates.io-publishable-f46623)](https://crates.io/crates/bedlam)

<!-- simit:badges:end -->

Open, self-hosted answer to [boat.dev](https://boat.dev): per-second microVM sandboxes for coding agents,
on your own hardware. Formerly `mutiny`; the crates.io name was taken.

Bedlam boots [microsandbox](https://github.com/superradcompany/microsandbox) (libkrun) VMs on whichever host of a
small pool has capacity according to [AMC](https://github.com/caniko/amc) admission, and exposes them to agents
through a local stdio MCP server.

## pool.json

Read from `$XDG_CONFIG_HOME/bedlam/pool.json` (or pass `--pool <path>`). Weak hosts are excluded
upstream; Bedlam trusts `hosts`.

```json
{
  "user": "can",
  "localHost": "atlas",
  "contract": "sandbox",
  "hosts": [{"name": "atlas", "address": "192.168.178.2", "sshPort": 22}],
  "secretFiles": {"GITHUB_TOKEN": "/run/user/1000/agenix/github-token"}
}
```

`homeModules.default` renders this file and installs `bedlam` wrapped with `amc`, `ssh` and `systemd` on `PATH`:
`programs.bedlam.{enable, amcPackage, localHost, poolHosts, secretFiles, runtimeDir}`; the wrapped binary is
`programs.bedlam.finalPackage`.

Hosts take an optional `"priority"` (u8, default 0, e.g. 1 for workers): placement prefers the highest priority, then the most free budget, then `localHost`.

Every host needs `bedlam` and `amc` (the `amc admission exec` CLI) on its non-interactive `PATH`, a shared `pool.json`, and key-based SSH.

## Commands

- `bedlam host status --json` - `{host, eligible, freeBudgetMiB, memAvailableMiB, memFullPsi, onAc, reason?}`.
  Missing or duplicate readings, or no read-write `/dev/kvm`, make the host ineligible.
- `bedlam host run --spec '{"repo":"o/r","ref":"main","cpus":2,"memoryMiB":2048}'` - start a sandbox here, print `{"id":"<host>:<name>"}`.
- `bedlam host exec|read|write|fork|snapshot|destroy|list` - operate on local sandboxes by name (`write` takes content on stdin).
- `bedlam mcp` - stdio MCP server.

## MCP tools

`pool_status`, `sandbox_create {repo, ref, cpus?, memoryMiB?}`, `exec {id, command}`, `read_file {id, path}`,
`write_file {id, path, content}`, `fork {id}`, `snapshot {id}`, `destroy {id}`, `list`.

`sandbox_create` queries all hosts in parallel (12 s cap), drops ineligible or unreachable ones, picks the most
free AMC budget (ties go to the local host) and runs `bedlam host run` there. Other tools route by the id's host prefix.

## AMC lease semantics

`host run` re-execs itself in a transient `systemd-run --user --unit bedlam-<name>` and returns once the VM is ready.
That unit queues for AMC admission via `amc admission exec --queue` (contract from `pool.json`, memory = VM RAM + 512 MiB, 5 min wait) and holds the
lease for the VM's lifetime, independent of the launching SSH session. Destroying the sandbox (or stopping the unit)
ends the VM and releases the lease.

## Guest

The guest is `docker.io/nixos/nix`, pinned by digest (Nix 2.35.4), with a 16 GiB root disk and
`experimental-features = nix-command flakes`. The repo is cloned to `/work/repo`; agents run toolchains with
`nix develop -c ...` via `exec` (cache.nixos.org only). No host directory is mounted, so untrusted guest code cannot poison host caches
(a leftover `sccacheDir` in an old `pool.json` is ignored).

Egress uses microsandbox's default policy (the `Public` profile): public internet only, private/LAN ranges and the host
are denied. Bedlam sets no network policy of its own.

Secrets from `secretFiles` go through microsandbox's secret proxy: the guest sees a placeholder, and the real value is
substituted only in the `Authorization` header of requests to `github.com` / `api.github.com`, and only after the proxy
has verified the server's TLS identity. Git in the guest authenticates through a global credential helper that sends
`x-access-token:$GITHUB_TOKEN` as Basic auth (GitHub's git smart-HTTP rejects `Bearer`); the proxy substitutes inside
the base64 credentials, so clones, fetches and pushes all work.

## Capacity and validation

A host's free budget is the AMC `admission status` of the user's ledger; if the shared-host broker socket
(`/run/amc-host/admission.sock`) exists it is the lower of that and `admission host-status`, and an unreadable
broker makes the host ineligible.

Sandbox names must be `bd-` prefixed with `[A-Za-z0-9_.-]`, for every verb and for `forkOf`; `repo` must be exactly
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
