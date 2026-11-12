# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-12

- Rename the project from `mutiny` to `bedlam` because the crates.io name `mutiny` is taken: crate, binary, MCP server, Home Manager module (`programs.bedlam`), config directory (`$XDG_CONFIG_HOME/bedlam/pool.json`), transient units (`bedlam-<name>`), secret environment variables (`BEDLAM_SECRET_*`) and sandbox name prefix (`bd-`).
- Upgrading from `mutiny`: destroy running `mu-*` sandboxes first (bedlam neither lists nor manages them, and their `mutiny-*` units would keep their AMC leases), then move `$XDG_CONFIG_HOME/mutiny/pool.json` to `$XDG_CONFIG_HOME/bedlam/pool.json` unless Home Manager renders it.
- `homeModules.default`: `programs.bedlam` installs the binary wrapped with `amc`, `ssh` and `systemd` on `PATH` and renders `pool.json` from `poolHosts`, `secretFiles` and `runtimeDir`.

## [0.1.1] - 2026-11-03

- Vendor the microsandbox runtime crates by fixed-output hash, so consumers evaluate Mutiny without import-from-derivation.

## [0.1.0] - 2026-11-03

- Pooled microVM sandboxes for coding agents: microsandbox (libkrun) VMs on the pool host with the most free AMC budget, behind a stdio MCP server (`pool_status`, `sandbox_create`, `exec`, `read_file`, `write_file`, `fork`, `snapshot`, `destroy`, `list`).
- Placement from per-host readings (user and shared-host AMC budget, MemAvailable, memory PSI, AC power); any missing reading makes a host ineligible.
- Sandboxes queue for AMC admission through `amc admission exec` and hold the lease for the VM's lifetime.
- Guest is a digest-pinned `nixos/nix` image with flakes enabled and a 16 GiB root disk; egress is public-only (microsandbox default policy).
- Secrets from `secretFiles` reach the guest only as placeholders, substituted in the Authorization header for github.com and api.github.com over verified TLS.
- Sandbox names must be `mu-` prefixed and repos exactly `owner/name`; pool SSH pins host keys.
- Runtime (`msb`, `libkrunfw`, `agentd`) is packaged from source in the flake and wrapped into the binary on Linux.

[Unreleased]: https://github.com/caniko/bedlam/compare/0.2.0...HEAD
[0.2.0]: https://github.com/caniko/bedlam/compare/0.1.1...0.2.0
