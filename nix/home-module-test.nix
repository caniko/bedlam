# Evaluates the home module against stub Home Manager options and checks the
# rendered pool.json; fails at evaluation time.
{
  pkgs,
  module,
}: let
  inherit (pkgs) lib;
  stub = lib.mkOption {type = lib.types.anything;};
  pool =
    builtins.fromJSON
    (lib.evalModules {
      modules = [
        module
        {
          options.home.username = stub;
          options.home.packages = stub;
          options.xdg.configFile = stub;
          config = {
            _module.args = {inherit pkgs;};
            home.username = "tester";
            programs.mutiny = {
              enable = true;
              amcPackage = pkgs.hello;
              localHost = "a";
              poolHosts = [
                {
                  name = "a";
                  address = "10.0.0.1";
                }
                {
                  name = "b";
                  address = "10.0.0.2";
                  sshPort = 2222;
                  priority = 2;
                }
              ];
              secretFiles.GITHUB_TOKEN = "\${XDG_RUNTIME_DIR}/token";
              runtimeDir = "/run/user/1000";
            };
          };
        }
      ];
    }).config.xdg.configFile."mutiny/pool.json".text;
in
  assert pool
  == {
    user = "tester";
    localHost = "a";
    contract = "sandbox";
    hosts = [
      {
        name = "a";
        address = "10.0.0.1";
        sshPort = 22;
        priority = 0;
      }
      {
        name = "b";
        address = "10.0.0.2";
        sshPort = 2222;
        priority = 2;
      }
    ];
    secretFiles.GITHUB_TOKEN = "/run/user/1000/token";
  };
    pkgs.runCommand "mutiny-home-module" {} "touch $out"
