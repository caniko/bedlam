# microsandbox runtime v0.7.13 (msb + guest agentd + libkrunfw), built from source.
# Mutiny's `microsandbox` SDK crate runs sandboxes via `msb` and needs the pair
# MSB_PATH + MSB_LIBKRUNFW_PATH; agentd is embedded into msb (MSB_EMBED_ARTIFACTS_DIR).
{pkgs}: let
  inherit (pkgs) lib;
  version = "0.7.13";
  src = pkgs.fetchFromGitHub {
    owner = "superradcompany";
    repo = "microsandbox";
    rev = "44c1e63176259e048f42893a5779de8b08fe0ec6"; # tag v${version}
    hash = "sha256-zvgK4ml8i1wPXGUHmLMbC9AoaK8rcV58oNNwjaKKy/c=";
  };
  # Fixed-output vendor (no import-from-derivation); one hash covers both crates of the workspace.
  cargoHash = "sha256-D//ypsFILmaCVZn8miSoYZpxaoj5dt/EIrthMQiVq2Y=";

  # Static musl guest init/agent (upstream: `just build-agentd`).
  agentd = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
    pname = "microsandbox-agentd";
    inherit version src cargoHash;
    cargoBuildFlags = ["-p" "microsandbox-agentd"];
    doCheck = false;
  };

  # Upstream's libkrunfw fork (kernel 6.12.113 + microsandbox virtio drivers). nixpkgs'
  # libkrunfw 5.5.0 lacks those patches and the file name msb looks for (libkrunfw.so.5.6.1).
  libkrunfw = pkgs.stdenv.mkDerivation {
    pname = "libkrunfw-microsandbox";
    version = "5.6.1";
    src = pkgs.fetchFromGitHub {
      owner = "superradcompany";
      repo = "libkrunfw";
      rev = "56bf194627a07996dc4ce21211fe86ada4c76080";
      hash = "sha256-ukrmizk+OqxydWsP4JZiEzgZtLpSjDLGo4pspusfyuk=";
    };
    kernelSrc = pkgs.fetchurl {
      url = "mirror://kernel/linux/kernel/v6.x/linux-6.12.113.tar.gz";
      hash = "sha256-QT99n8u9hcmAmHuMy2+yR+nKvZtf3MKS4twdVfQotj0=";
    };
    postPatch = ''
      substituteInPlace Makefile \
        --replace-fail 'curl --fail --location --retry 5 --retry-delay 2 $(KERNEL_REMOTE) -o $(KERNEL_TARBALL)' \
                       'ln -s $(kernelSrc) $(KERNEL_TARBALL)'
      patchShebangs scripts
    '';
    nativeBuildInputs = with pkgs; [flex bison bc cpio perl python3 python3.pkgs.pyelftools];
    buildInputs = [pkgs.elfutils];
    enableParallelBuilding = true;
    installPhase = ''
      install -Dm755 libkrunfw.so.5.6.1 $out/lib/libkrunfw.so.5.6.1
      ln -s libkrunfw.so.5.6.1 $out/lib/libkrunfw.so.5
      ln -s libkrunfw.so.5 $out/lib/libkrunfw.so
    '';
    meta.platforms = ["x86_64-linux" "aarch64-linux"];
  };

  msb = pkgs.rustPlatform.buildRustPackage {
    pname = "msb";
    inherit version src cargoHash;
    cargoBuildFlags = ["-p" "microsandbox-cli"];
    buildNoDefaultFeatures = true;
    buildFeatures = ["embed-binaries" "net" "ssh"];
    MSB_EMBED_ARTIFACTS_DIR = "${agentd}/bin";
    nativeBuildInputs = [pkgs.pkg-config];
    doCheck = false;
    meta.mainProgram = "msb";
  };
in {inherit agentd libkrunfw msb;}
