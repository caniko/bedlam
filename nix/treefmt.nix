{rustfmtPackage}: {pkgs, ...}: {
  projectRootFile = "flake.nix";

  programs.rustfmt = {
    enable = true;
    edition = "2024";
    package = rustfmtPackage;
  };

  programs.alejandra.enable = true;

  programs.taplo.enable = true;
}
