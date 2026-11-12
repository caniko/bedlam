# Bedlam vision

Bedlam is an open, self-hosted answer to hosted microVM sandbox services: it
boots per-second microsandbox (libkrun) VMs on whichever host of a small pool
has capacity, and exposes them to coding agents through a local stdio MCP
server. Formerly `mutiny`; the crates.io name was taken. The point is that agent
code execution happens on hardware you own, with your own accounting and your
own access rules.

## Problem

Hosted sandbox APIs are convenient, but they move code execution and its output
off your machine, into someone else's scheduler and trust boundary. Running
agents in plain containers on the workstation has the opposite problem: they
share the host kernel and inherit ambient credentials. What is missing locally is
a pool abstraction — create, exec in, fork, snapshot, and destroy disposable
VMs, placed where there is real memory and power budget, without handing secrets
to the guest.

## Principles

- **Placement follows live readings and fails closed.** A host is eligible only
  if its AMC budget, `MemAvailable`, memory PSI, and AC state are all readable;
  missing or duplicate readings make it ineligible. Priority, then free budget,
  then the local host decide the winner.
- **The lease lasts as long as the VM.** `host run` re-execs into a transient
  `systemd-run --user` unit that queues for AMC admission and holds the lease for
  the VM's lifetime, independent of the launching SSH session.
- **Secrets stay placeholders.** Values from `secretFiles` reach the guest only
  as placeholders, substituted in the `Authorization` header for github.com and
  api.github.com over verified TLS.
- **Validate at the trust boundary.** Sandbox names must be `bd-` prefixed, repos
  exactly `owner/name`, and pool SSH pins host keys.
- **Boring packaging.** The microsandbox runtime is packaged from source and its
  crates are vendored by fixed-output hash, so consumers evaluate Bedlam without
  import-from-derivation.
- **No surprise egress.** The guest is a digest-pinned `nixos/nix` image with
  flakes enabled and public-only egress under the runtime's default policy.

## Scope

- pool placement and per-host status (`bedlam host status|run|exec|read|write|
  fork|snapshot|destroy|list`)
- stdio MCP server with `pool_status`, `sandbox_create`, and sandbox operations
- AMC admission integration and lease lifetime
- secret placeholder proxy
- runtime packaging (msb, libkrunfw, agentd) and the flake outputs
- KVM boot coverage for the packaged runtime

## Direction

The project is at 0.1.x and the recent work is qualification: vendoring runtime
crates by hash instead of IFD, packaging the microsandbox runtime, documenting
the guest, egress, and budget behaviour, and adding a cargo-deny policy.

## Place in the fleet

Canix declares Bedlam as a flake input, renders `pool.json` at
`$XDG_CONFIG_HOME/bedlam/pool.json`, and lists Bedlam among the MCP servers that
Atlas agent harnesses load, so agents can create and use sandboxes through the
local server.
