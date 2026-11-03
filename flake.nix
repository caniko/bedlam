{
  description = "Mutiny: pooled microVM sandboxes for coding agents";

  inputs = {
    harbor.url = "git+https://github.com/caniko/harbor.git?ref=feat/harbor-monorepo-components&rev=7d99eb50c52d0a941e2996b97c469b32a7657ef4";
    nixpkgs.follows = "harbor/nixpkgs";
    rust-overlay.follows = "harbor/rust-overlay";
    crane.follows = "harbor/crane";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = {
    self,
    harbor,
    nixpkgs,
    rust-overlay,
    crane,
    flake-utils,
    treefmt-nix,
    git-hooks,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };

      toolchain = harbor.lib.rust.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib rustToolchain;
      fmtToolchain = harbor.lib.rust.mkToolchain {
        inherit pkgs;
        toolchainProfile = "nightly";
      };
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      msbRuntime = import ./nix/msb.nix {inherit pkgs;};
      # On Linux, wrapped for run time: msb + libkrunfw come from the store. amc is
      # NOT bundled (supplied on PATH). libkrunfw is Linux-only; other systems get
      # the bare binary and bring their own runtime.
      inherit (pkgs.stdenv.hostPlatform) isLinux;
      package = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
          nativeBuildInputs = pkgs.lib.optional isLinux pkgs.makeWrapper;
          postInstall = pkgs.lib.optionalString isLinux ''
            wrapProgram $out/bin/mutiny \
              --set-default MSB_PATH ${msbRuntime.msb}/bin/msb \
              --set-default MSB_LIBKRUNFW_PATH ${msbRuntime.libkrunfw}/lib/libkrunfw.so.5.6.1
          '';
        });
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix {rustfmtPackage = fmtToolchain.rustToolchain;});
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          inherit rustToolchain;
        };
      };
    in {
      packages =
        {
          default = package;
        }
        // pkgs.lib.optionalAttrs isLinux {
          inherit (msbRuntime) msb libkrunfw agentd;
        };
      formatter = treefmtEval.config.build.wrapper;
      checks = {
        default = package;
        formatting = treefmtEval.config.build.check self;
        clippy = craneLib.cargoClippy (commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
          });
        fmt = craneLib.cargoFmt {inherit src;};
      };
      devShells.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs;
          [
            treefmtEval.config.build.wrapper
            cargo-audit
            cargo-deny
            cargo-llvm-cov
            cargo-nextest
            jq
            pre-commit
            rust-analyzer
            taplo
          ]
          ++ pre-commit-check.enabledPackages;
        shellHook = pre-commit-check.shellHook;
      };

      apps.local-check-fast = {
        type = "app";
        program = let
          script = pkgs.writeShellApplication {
            name = "local-check-fast";
            runtimeInputs = with pkgs; [
              cargo-deny
              git
              jq
              rustToolchain
            ];
            text = ''
              set -euo pipefail
              cargo test --workspace --all-features
              cargo clippy --workspace --all-targets --all-features -- --deny warnings
              cargo deny check bans licenses sources
            '';
          };
        in "${script}/bin/local-check-fast";
        meta.description = "Run fast local validation checks";
      };
    });
}
