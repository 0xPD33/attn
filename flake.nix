{
  description = "attn local attention ledger";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    home-manager = {
      url = "github:nix-community/home-manager/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, home-manager, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f nixpkgs.legacyPackages.${system});
      defaultConfig = builtins.readFile ./config/default.toml;
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "attn";
          version = "0.1.0";

          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let
                name = baseNameOf path;
              in
              !(name == "target"
                || name == ".git"
                || name == ".agents"
                || name == ".codex");
          };

          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/attn";
          meta.description = "attn local attention ledger";
        };
      });

      checks = forAllSystems (pkgs: {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;

        homeManagerModule =
          (home-manager.lib.homeManagerConfiguration {
            inherit pkgs;
            modules = [
              self.homeManagerModules.default
              ({ ... }: {
                home.username = "attn";
                home.homeDirectory = "/tmp/attn-home";
                home.stateVersion = "24.05";

                programs.attn.enable = true;
                programs.attn.daemon.enable = true;
                programs.attn.niriPackage = null;
                programs.attn.quickshell.enable = true;
              })
            ];
          }).activationPackage;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.clippy
            pkgs.pkg-config
            pkgs.rustc
            pkgs.rustfmt
            pkgs.sqlite
          ];

          buildInputs = [
            pkgs.sqlite
          ];
        };
      });

      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.attn;
        in
        {
          options.programs.attn = {
            enable = lib.mkEnableOption "attn local attention ledger";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "attn package to install.";
            };

            niriPackage = lib.mkOption {
              type = lib.types.nullOr lib.types.package;
              default = pkgs.niri;
              description = ''
                Niri package whose bin directory is added to the attn service
                PATH. Set to null if niri is provided by the user service
                environment some other way.
              '';
            };

            configText = lib.mkOption {
              type = lib.types.lines;
              default = defaultConfig;
              description = "Contents of ~/.config/attn/config.toml.";
            };

            daemon.enable = lib.mkEnableOption "attn user daemon";

            quickshell.enable = lib.mkEnableOption ''
              installation of the attn Quickshell indicator component under
              ~/.config/quickshell/attn/AttnIndicator.qml. If another module
              manages ~/.config/quickshell as a whole tree, add this component
              to that source tree instead and leave this option disabled.
            '';
          };

          config = lib.mkIf cfg.enable {
            assertions = [
              {
                assertion = !(cfg.quickshell.enable && (config.xdg.configFile ? "quickshell"));
                message = ''
                  programs.attn.quickshell.enable cannot install
                  quickshell/attn/AttnIndicator.qml while xdg.configFile."quickshell"
                  already manages the whole Quickshell tree. Add AttnIndicator.qml
                  to that source tree and import it from the bar instead.
                '';
              }
            ];

            home.packages = [ cfg.package ];

            xdg.configFile."attn/config.toml".text = cfg.configText;

            xdg.configFile."quickshell/attn/AttnIndicator.qml" = lib.mkIf cfg.quickshell.enable {
              source = ./quickshell/AttnIndicator.qml;
            };

            systemd.user.services.attn = lib.mkIf cfg.daemon.enable {
              Unit = {
                Description = "attn local attention ledger";
                After = [ "graphical-session.target" ];
                PartOf = [ "graphical-session.target" ];
              };

              Service = {
                ExecStart = "${cfg.package}/bin/attn daemon";
                Environment = lib.mkIf (cfg.niriPackage != null) [
                  "PATH=${lib.makeBinPath [ cfg.niriPackage ]}"
                ];
                Restart = "on-failure";
                RestartSec = 3;
              };

              Install.WantedBy = [ "graphical-session.target" ];
            };
          };
        };
    };
}
