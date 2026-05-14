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
      defaultConfig = ''
        poll_interval_secs = 60
        idle_after_secs = 300
        socket_path = "$XDG_RUNTIME_DIR/attn.sock"
        state_path = "~/.local/state/attn/attn.sqlite"

        [apps.watch]
        coding = [
          "code", "code-insiders", "vscodium", "codium",
          "cursor", "windsurf", "zed", "dev.zed.Zed", "antigravity",
          "sublime_text", "lapce", "android-studio", "jetbrains-toolbox",
          "intellij-idea-ultimate", "intellij-idea-community", "webstorm",
          "pycharm", "pycharm-community", "goland", "rustrover", "phpstorm",
          "rubymine", "clion", "datagrip", "rider", "fleet",
        ]
        terminal = [
          "com.mitchellh.ghostty", "ghostty", "wezterm", "kitty", "alacritty",
          "foot", "gnome-terminal", "konsole", "xterm", "urxvt", "terminator",
          "tilix", "warp", "dev.warp.Warp",
        ]
        chat = [
          "discord", "vesktop", "signal", "signal-desktop", "slack",
          "telegram-desktop", "org.telegram.desktop", "element", "element-desktop",
          "thunderbird", "betterbird", "geary", "evolution", "mailspring",
          "teams-for-linux", "zoom", "us.zoom.Zoom",
        ]
        design = [
          "figma-linux", "inkscape", "org.inkscape.Inkscape",
          "gimp", "org.gimp.GIMP", "krita", "org.kde.krita",
          "blender", "org.blender.Blender", "godot", "org.godotengine.Godot",
        ]
        productivity = [
          "obsidian", "md.obsidian.Obsidian", "logseq", "com.logseq.Logseq",
          "joplin", "notion-app", "anytype", "zettlr",
        ]
        video = [
          "mpv", "io.mpv.Mpv", "vlc", "org.videolan.VLC",
          "celluloid", "smplayer", "haruna",
        ]
        games = [
          "steam", "com.valvesoftware.Steam", "lutris", "net.lutris.Lutris",
          "heroic", "com.heroicgameslauncher.hgl", "bottles",
          "com.usebottles.bottles", "itch", "io.itch.itch",
        ]
        music = [
          "spotify", "com.spotify.Client", "rhythmbox", "elisa",
          "amberol", "io.bassi.Amberol", "audacious",
        ]

        [domains.watch]
        coding = [
          "github.com", "gist.github.com", "gitlab.com", "bitbucket.org", "codeberg.org",
          "stackoverflow.com", "stackexchange.com", "serverfault.com", "superuser.com", "askubuntu.com",
          "vercel.com", "netlify.com", "cloudflare.com", "developers.cloudflare.com",
          "railway.app", "railway.com", "fly.io", "render.com",
          "supabase.com", "planetscale.com", "neon.tech",
          "console.aws.amazon.com", "cloud.google.com", "portal.azure.com",
          "npmjs.com", "pypi.org", "crates.io", "rubygems.org", "packagist.org", "jsr.io",
          "developer.mozilla.org", "web.dev", "caniuse.com", "regex101.com", "godbolt.org",
          "docs.python.org", "docs.rs", "doc.rust-lang.org", "nodejs.org",
          "react.dev", "nextjs.org", "vuejs.org", "svelte.dev",
          "tailwindcss.com", "typescriptlang.org",
          "learn.microsoft.com", "developers.google.com", "developer.apple.com",
          "kubernetes.io", "helm.sh",
          "nixos.org", "wiki.nixos.org", "search.nixos.org", "huggingface.co",
        ]
        ai = [
          "chatgpt.com", "claude.ai", "gemini.google.com",
          "anthropic.com", "openai.com", "platform.openai.com",
          "poe.com", "perplexity.ai", "you.com", "mistral.ai",
          "deepseek.com", "chat.deepseek.com", "kimi.moonshot.cn",
          "grok.com", "x.ai", "copilot.microsoft.com",
          "console.anthropic.com", "aistudio.google.com",
        ]
        design = [
          "figma.com", "sketch.com", "framer.com", "behance.net", "dribbble.com",
          "midjourney.com", "unsplash.com", "pexels.com",
          "excalidraw.com", "tldraw.com", "app.diagrams.net",
        ]
        productivity = [
          "notion.so", "linear.app", "atlassian.net",
          "jira.atlassian.com", "confluence.atlassian.com",
          "monday.com", "airtable.com", "trello.com", "asana.com",
          "clickup.com", "shortcut.com", "height.app", "todoist.com",
        ]
        chat = [
          "discord.com", "slack.com", "teams.microsoft.com",
          "web.whatsapp.com", "web.telegram.org", "messenger.com",
          "signal.org", "element.io", "app.element.io",
        ]
        meeting = [
          "meet.google.com", "zoom.us", "app.zoom.us", "whereby.com", "around.co",
        ]
        video = [
          "youtube.com", "youtu.be", "m.youtube.com",
          "vimeo.com", "netflix.com", "twitch.tv",
          "primevideo.com", "hulu.com", "disneyplus.com",
          "hbomax.com", "max.com", "peacocktv.com", "paramount.plus",
          "kick.com", "dailymotion.com",
        ]
        scroll = [
          "reddit.com", "old.reddit.com",
          "x.com", "twitter.com",
          "tiktok.com", "instagram.com", "facebook.com",
          "threads.net", "threads.com", "bsky.app",
          "tumblr.com", "9gag.com",
          "news.ycombinator.com", "lemmy.world", "kbin.social",
        ]

        [browsers.helium]
        app_ids = ["helium", "net.imput.helium"]
        history_paths = [
          "~/.config/net.imput.helium/*/History",
          "~/.var/app/net.imput.helium/config/net.imput.helium/*/History",
        ]
        kind = "chromium"

        [browsers.brave]
        app_ids = ["brave-browser", "brave", "com.brave.Browser"]
        history_paths = [
          "~/.config/BraveSoftware/Brave-Browser/*/History",
          "~/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser/*/History",
          "~/snap/brave/current/.config/BraveSoftware/Brave-Browser/*/History",
        ]
        kind = "chromium"

        [browsers.chrome]
        app_ids = ["google-chrome", "chrome", "com.google.Chrome"]
        history_paths = [
          "~/.config/google-chrome/*/History",
          "~/.var/app/com.google.Chrome/config/google-chrome/*/History",
        ]
        kind = "chromium"

        [browsers.chromium]
        app_ids = ["chromium", "chromium-browser", "org.chromium.Chromium", "Chromium-browser"]
        history_paths = [
          "~/.config/chromium/*/History",
          "~/.var/app/org.chromium.Chromium/config/chromium/*/History",
          "~/snap/chromium/common/chromium/*/History",
        ]
        kind = "chromium"

        [display]
        domains_show_top = 12
        domains_min_seconds = 30

        [terminals]
        poll_secs = 10
        [terminals.apps]
        ai = ["claude", "codex", "aichat"]
        editor = ["nvim", "vim", "hx", "helix", "emacs"]
      '';
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
