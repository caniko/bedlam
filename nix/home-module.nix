# Home Manager module: the wrapped `bedlam` binary and its pool.json.
self: {
  config,
  lib,
  pkgs,
  ...
}: let
  inherit (lib) mkOption types;
  cfg = config.programs.bedlam;
  resolve =
    if cfg.runtimeDir == null
    then lib.id
    else lib.replaceStrings ["\${XDG_RUNTIME_DIR}"] [cfg.runtimeDir];
in {
  options.programs.bedlam = {
    enable = lib.mkEnableOption "Bedlam pooled microVM sandboxes";
    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "bedlam.packages.\${system}.default";
    };
    amcPackage = mkOption {
      type = types.package;
      description = "AMC providing `amc admission exec`; sandbox units run under the user manager, whose PATH lacks the profile.";
    };
    finalPackage = mkOption {
      type = types.package;
      readOnly = true;
      description = "`package` with amc, ssh and systemd on PATH.";
    };
    user = mkOption {
      type = types.str;
      default = config.home.username;
      defaultText = lib.literalExpression "config.home.username";
    };
    localHost = mkOption {
      type = types.str;
      description = "This host's name in `poolHosts`.";
    };
    contract = mkOption {
      type = types.str;
      default = "sandbox";
      description = "AMC contract sandboxes queue under.";
    };
    poolHosts = mkOption {
      type = types.listOf (types.submodule {
        options = {
          name = mkOption {type = types.str;};
          address = mkOption {type = types.str;};
          sshPort = mkOption {
            type = types.port;
            default = 22;
          };
          priority = mkOption {
            type = types.ints.u8;
            default = 0;
            description = "Placement prefers the highest priority.";
          };
        };
      });
      description = "Every pool member, this host included. Bedlam trusts the list.";
    };
    secretFiles = mkOption {
      type = types.attrsOf types.str;
      default = {};
      description = "Secret name to the file holding it; contents never enter the store.";
    };
    runtimeDir = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "/run/user/1000";
      description = "Substituted for a literal `\${XDG_RUNTIME_DIR}` in `secretFiles`, which Bedlam reads verbatim.";
    };
  };
  config = lib.mkIf cfg.enable {
    programs.bedlam.finalPackage = pkgs.symlinkJoin {
      name = "bedlam-wrapped";
      paths = [cfg.package];
      nativeBuildInputs = [pkgs.makeWrapper];
      postBuild = "wrapProgram $out/bin/bedlam --prefix PATH : ${lib.makeBinPath [cfg.amcPackage pkgs.openssh pkgs.systemd]}";
      meta.mainProgram = "bedlam";
    };
    home.packages = [cfg.finalPackage];
    xdg.configFile."bedlam/pool.json".text = builtins.toJSON {
      inherit (cfg) user localHost contract;
      hosts = cfg.poolHosts;
      secretFiles = lib.mapAttrs (_: resolve) cfg.secretFiles;
    };
  };
}
