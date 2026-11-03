# Changelog

## 0.1.0 - 2026-11-03

- Pooled microVM sandboxes for coding agents: microsandbox (libkrun) VMs on the pool host with the most free AMC budget, behind a stdio MCP server (`pool_status`, `sandbox_create`, `exec`, `read_file`, `write_file`, `fork`, `snapshot`, `destroy`, `list`).
- Placement from per-host readings (user and shared-host AMC budget, MemAvailable, memory PSI, AC power); any missing reading makes a host ineligible.
- Sandboxes queue for AMC admission through `amc admission exec` and hold the lease for the VM's lifetime.
- Guest is a digest-pinned `nixos/nix` image with flakes enabled and a 16 GiB root disk; egress is public-only (microsandbox default policy).
- Secrets from `secretFiles` reach the guest only as placeholders, substituted in the Authorization header for github.com and api.github.com over verified TLS.
- Sandbox names must be `mu-` prefixed and repos exactly `owner/name`; pool SSH pins host keys.
- Runtime (`msb`, `libkrunfw`, `agentd`) is packaged from source in the flake and wrapped into the binary on Linux.
