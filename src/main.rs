use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use glob::glob;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration as StdDuration, Instant};
use url::Url;

mod focus;

const DEFAULT_CONFIG_PATH: &str = "~/.config/attn/config.toml";
const DEFAULT_STATE_PATH: &str = "~/.local/state/attn/attn.sqlite";
const DEFAULT_SOCKET_PATH: &str = "$XDG_RUNTIME_DIR/attn.sock";
const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;
const SOCKET_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(5);
const DEFAULT_CONFIG_TOML: &str = include_str!("../config/default.toml");

fn main() {
    if let Err(error) = run() {
        eprintln!("attn: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("daemon") => run_daemon(),
        Some("status") => {
            let json = args.any(|arg| arg == "--json");
            if !json {
                bail!("status currently requires --json");
            }
            let config = Config::load_default()?;
            let response = socket_request(&config.socket_path, "status\n")?;
            println!("{response}");
            Ok(())
        }
        Some("reload") => {
            let config = Config::load_default()?;
            let response = socket_request(&config.socket_path, "reload\n")?;
            println!("{response}");
            Ok(())
        }
        Some("break-start") => {
            let config = Config::load_default()?;
            let response = socket_request(&config.socket_path, "break_start\n")?;
            println!("{response}");
            Ok(())
        }
        Some("break-end") => {
            let config = Config::load_default()?;
            let response = socket_request(&config.socket_path, "break_end\n")?;
            println!("{response}");
            Ok(())
        }
        Some("set-breaks") => {
            let mut enabled: Option<bool> = None;
            let mut interval: Option<i64> = None;
            let mut min_break: Option<i64> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--enabled=") {
                    enabled = Some(v == "true" || v == "1");
                } else if let Some(v) = arg.strip_prefix("--interval=") {
                    interval = v.parse().ok();
                } else if let Some(v) = arg.strip_prefix("--min-break=") {
                    min_break = v.parse().ok();
                }
            }
            let cfg = Config::load_default()?;
            let payload = format!(
                "set_breaks {} {} {}\n",
                if enabled.unwrap_or(cfg.breaks.enabled) { 1 } else { 0 },
                interval.unwrap_or(cfg.breaks.interval_secs),
                min_break.unwrap_or(cfg.breaks.min_break_secs),
            );
            let response = socket_request(&cfg.socket_path, &payload)?;
            println!("{response}");
            Ok(())
        }
        Some("init") => {
            let mut force = false;
            let mut merge = false;
            for arg in args {
                match arg.as_str() {
                    "--force" => force = true,
                    "--merge" => merge = true,
                    other => bail!("unknown init option: {other}"),
                }
            }
            init_config(force, merge)
        }
        Some("doctor") => run_doctor(),
        Some("--help") | Some("-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => bail!("unknown command: {command}"),
    }
}

fn print_help() {
    println!(
        "attn\n\nUsage:\n  attn daemon\n  attn status --json\n  attn reload\n  attn break-start\n  attn break-end\n  attn set-breaks [--enabled=true|false] [--interval=SECS] [--min-break=SECS]\n  attn init [--force|--merge]\n  attn doctor"
    );
}

fn init_config(force: bool, merge: bool) -> Result<()> {
    let path = expand_path(DEFAULT_CONFIG_PATH);
    if path.exists() && merge {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let user_config = toml::from_str::<Config>(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        let mut config = bundled_default_config()?;
        config.apply_user_config(user_config);
        let merged = toml::to_string_pretty(&config).context("failed to serialize merged config")?;
        ensure_parent_dir(&path)?;
        fs::write(&path, merged)
            .with_context(|| format!("failed to write config {}", path.display()))?;
        println!("merged defaults into {}", path.display());
        return Ok(());
    }
    if path.exists() && !force {
        bail!(
            "config already exists at {}; pass --merge to add bundled defaults or --force to overwrite",
            path.display()
        );
    }
    ensure_parent_dir(&path)?;
    fs::write(&path, DEFAULT_CONFIG_TOML)
        .with_context(|| format!("failed to write config {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_poll_interval_secs")]
    poll_interval_secs: u64,
    #[serde(default = "default_idle_after_secs")]
    idle_after_secs: i64,
    #[serde(default = "default_socket_path")]
    socket_path: PathBuf,
    #[serde(default = "default_state_path")]
    state_path: PathBuf,
    #[serde(default = "default_apps_config")]
    apps: AppsConfig,
    #[serde(default = "default_domains_config")]
    domains: DomainsConfig,
    #[serde(default = "default_browsers")]
    browsers: BTreeMap<String, BrowserConfig>,
    #[serde(default)]
    display: DisplayConfig,
    #[serde(default = "default_budgets")]
    budgets: BTreeMap<String, BudgetEntry>,
    #[serde(default = "default_terminals_config")]
    terminals: TerminalsConfig,
    #[serde(default)]
    breaks: BreaksConfig,
    #[serde(default)]
    focus_source: FocusSourceConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BreaksConfig {
    #[serde(default = "default_breaks_enabled")]
    enabled: bool,
    #[serde(default = "default_breaks_interval_secs")]
    interval_secs: i64,
    #[serde(default = "default_breaks_min_break_secs")]
    min_break_secs: i64,
}

impl Default for BreaksConfig {
    fn default() -> Self {
        Self {
            enabled: default_breaks_enabled(),
            interval_secs: default_breaks_interval_secs(),
            min_break_secs: default_breaks_min_break_secs(),
        }
    }
}

fn default_breaks_enabled() -> bool { true }
fn default_breaks_interval_secs() -> i64 { 3600 }
fn default_breaks_min_break_secs() -> i64 { 300 }

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FocusSourceConfig {
    #[serde(default = "default_focus_source_kind")]
    kind: String,
}

impl Default for FocusSourceConfig {
    fn default() -> Self {
        Self { kind: default_focus_source_kind() }
    }
}

fn default_focus_source_kind() -> String {
    "auto".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TerminalsConfig {
    #[serde(default = "default_terminal_poll_secs")]
    poll_secs: u64,
    #[serde(default)]
    apps: BTreeMap<String, Vec<String>>,
}

impl Default for TerminalsConfig {
    fn default() -> Self {
        default_terminals_config()
    }
}

fn default_terminal_poll_secs() -> u64 {
    10
}

fn default_terminals_config() -> TerminalsConfig {
    let mut apps = BTreeMap::new();
    apps.insert(
        "ai".to_string(),
        vec!["claude".to_string(), "codex".to_string(), "aichat".to_string()],
    );
    apps.insert(
        "editor".to_string(),
        vec![
            "nvim".to_string(),
            "vim".to_string(),
            "hx".to_string(),
            "helix".to_string(),
            "emacs".to_string(),
        ],
    );
    TerminalsConfig {
        poll_secs: default_terminal_poll_secs(),
        apps,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BudgetEntry {
    #[serde(default)]
    daily_budget_secs: i64,
}

fn default_budgets() -> BTreeMap<String, BudgetEntry> {
    BTreeMap::new()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DisplayConfig {
    #[serde(default = "default_domains_show_top")]
    domains_show_top: usize,
    #[serde(default = "default_domains_min_seconds")]
    domains_min_seconds: i64,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            domains_show_top: default_domains_show_top(),
            domains_min_seconds: default_domains_min_seconds(),
        }
    }
}

fn default_domains_show_top() -> usize {
    12
}

fn default_domains_min_seconds() -> i64 {
    30
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppsConfig {
    #[serde(default = "default_app_watch")]
    watch: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DomainsConfig {
    #[serde(default = "default_domain_watch")]
    watch: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BrowserConfig {
    #[serde(default)]
    app_ids: Vec<String>,
    #[serde(default)]
    history_paths: Vec<String>,
    #[serde(default)]
    kind: String,
}

fn default_poll_interval_secs() -> u64 {
    60
}

fn default_idle_after_secs() -> i64 {
    300
}

fn default_apps_config() -> AppsConfig {
    AppsConfig {
        watch: default_app_watch(),
    }
}

fn default_app_watch() -> BTreeMap<String, Vec<String>> {
    let mut watch = BTreeMap::new();
    watch.insert(
        "coding".to_string(),
        vec![
            "code".to_string(),
            "code-insiders".to_string(),
            "vscodium".to_string(),
            "codium".to_string(),
            "cursor".to_string(),
            "windsurf".to_string(),
            "zed".to_string(),
            "dev.zed.Zed".to_string(),
            "antigravity".to_string(),
            "sublime_text".to_string(),
            "lapce".to_string(),
            "android-studio".to_string(),
            "jetbrains-toolbox".to_string(),
            "jetbrains-idea".to_string(),
            "intellij-idea-ultimate".to_string(),
            "intellij-idea-community".to_string(),
            "webstorm".to_string(),
            "pycharm".to_string(),
            "pycharm-community".to_string(),
            "goland".to_string(),
            "rustrover".to_string(),
            "phpstorm".to_string(),
            "rubymine".to_string(),
            "clion".to_string(),
            "datagrip".to_string(),
            "rider".to_string(),
            "fleet".to_string(),
        ],
    );
    watch.insert(
        "terminal".to_string(),
        vec![
            "com.mitchellh.ghostty".to_string(),
            "ghostty".to_string(),
            "wezterm".to_string(),
            "kitty".to_string(),
            "alacritty".to_string(),
            "foot".to_string(),
            "org.wezfurlong.wezterm".to_string(),
            "gnome-terminal".to_string(),
            "konsole".to_string(),
            "xterm".to_string(),
            "urxvt".to_string(),
            "terminator".to_string(),
            "tilix".to_string(),
            "warp".to_string(),
            "dev.warp.Warp".to_string(),
        ],
    );
    watch.insert(
        "chat".to_string(),
        vec![
            "discord".to_string(),
            "vesktop".to_string(),
            "signal".to_string(),
            "signal-desktop".to_string(),
            "slack".to_string(),
            "telegram-desktop".to_string(),
            "org.telegram.desktop".to_string(),
            "element".to_string(),
            "element-desktop".to_string(),
            "im.riot.Riot".to_string(),
            "thunderbird".to_string(),
            "betterbird".to_string(),
            "geary".to_string(),
            "evolution".to_string(),
            "mailspring".to_string(),
            "teams-for-linux".to_string(),
            "zoom".to_string(),
            "us.zoom.Zoom".to_string(),
        ],
    );
    watch.insert(
        "design".to_string(),
        vec![
            "figma-linux".to_string(),
            "inkscape".to_string(),
            "org.inkscape.Inkscape".to_string(),
            "gimp".to_string(),
            "org.gimp.GIMP".to_string(),
            "krita".to_string(),
            "org.kde.krita".to_string(),
            "blender".to_string(),
            "org.blender.Blender".to_string(),
            "godot".to_string(),
            "org.godotengine.Godot".to_string(),
        ],
    );
    watch.insert(
        "productivity".to_string(),
        vec![
            "obsidian".to_string(),
            "md.obsidian.Obsidian".to_string(),
            "logseq".to_string(),
            "com.logseq.Logseq".to_string(),
            "joplin".to_string(),
            "joplin-desktop".to_string(),
            "notion-app".to_string(),
            "notion-snap".to_string(),
            "anytype".to_string(),
            "zettlr".to_string(),
        ],
    );
    watch.insert(
        "video".to_string(),
        vec![
            "mpv".to_string(),
            "io.mpv.Mpv".to_string(),
            "vlc".to_string(),
            "org.videolan.VLC".to_string(),
            "celluloid".to_string(),
            "io.github.celluloid_player.Celluloid".to_string(),
            "smplayer".to_string(),
            "haruna".to_string(),
        ],
    );
    watch.insert(
        "games".to_string(),
        vec![
            "steam".to_string(),
            "com.valvesoftware.Steam".to_string(),
            "lutris".to_string(),
            "net.lutris.Lutris".to_string(),
            "heroic".to_string(),
            "com.heroicgameslauncher.hgl".to_string(),
            "bottles".to_string(),
            "com.usebottles.bottles".to_string(),
            "itch".to_string(),
            "io.itch.itch".to_string(),
        ],
    );
    watch.insert(
        "music".to_string(),
        vec![
            "spotify".to_string(),
            "com.spotify.Client".to_string(),
            "rhythmbox".to_string(),
            "org.gnome.Rhythmbox3".to_string(),
            "elisa".to_string(),
            "amberol".to_string(),
            "io.bassi.Amberol".to_string(),
            "audacious".to_string(),
        ],
    );
    watch
}

impl Default for AppsConfig {
    fn default() -> Self {
        default_apps_config()
    }
}

fn default_domains_config() -> DomainsConfig {
    DomainsConfig {
        watch: default_domain_watch(),
    }
}

fn default_domain_watch() -> BTreeMap<String, Vec<String>> {
    let mut watch = BTreeMap::new();
    watch.insert(
        "coding".to_string(),
        vec![
            "github.com".to_string(),
            "gist.github.com".to_string(),
            "gitlab.com".to_string(),
            "bitbucket.org".to_string(),
            "codeberg.org".to_string(),
            "stackoverflow.com".to_string(),
            "stackexchange.com".to_string(),
            "serverfault.com".to_string(),
            "superuser.com".to_string(),
            "askubuntu.com".to_string(),
            "vercel.com".to_string(),
            "netlify.com".to_string(),
            "cloudflare.com".to_string(),
            "developers.cloudflare.com".to_string(),
            "railway.app".to_string(),
            "railway.com".to_string(),
            "fly.io".to_string(),
            "render.com".to_string(),
            "supabase.com".to_string(),
            "planetscale.com".to_string(),
            "neon.tech".to_string(),
            "console.aws.amazon.com".to_string(),
            "cloud.google.com".to_string(),
            "portal.azure.com".to_string(),
            "npmjs.com".to_string(),
            "pypi.org".to_string(),
            "crates.io".to_string(),
            "rubygems.org".to_string(),
            "packagist.org".to_string(),
            "jsr.io".to_string(),
            "developer.mozilla.org".to_string(),
            "web.dev".to_string(),
            "caniuse.com".to_string(),
            "regex101.com".to_string(),
            "godbolt.org".to_string(),
            "docs.python.org".to_string(),
            "docs.rs".to_string(),
            "doc.rust-lang.org".to_string(),
            "nodejs.org".to_string(),
            "react.dev".to_string(),
            "nextjs.org".to_string(),
            "vuejs.org".to_string(),
            "svelte.dev".to_string(),
            "tailwindcss.com".to_string(),
            "typescriptlang.org".to_string(),
            "learn.microsoft.com".to_string(),
            "developers.google.com".to_string(),
            "developer.apple.com".to_string(),
            "kubernetes.io".to_string(),
            "helm.sh".to_string(),
            "nixos.org".to_string(),
            "wiki.nixos.org".to_string(),
            "search.nixos.org".to_string(),
            "huggingface.co".to_string(),
        ],
    );
    watch.insert(
        "ai".to_string(),
        vec![
            "chatgpt.com".to_string(),
            "claude.ai".to_string(),
            "gemini.google.com".to_string(),
            "anthropic.com".to_string(),
            "openai.com".to_string(),
            "platform.openai.com".to_string(),
            "poe.com".to_string(),
            "perplexity.ai".to_string(),
            "you.com".to_string(),
            "mistral.ai".to_string(),
            "deepseek.com".to_string(),
            "chat.deepseek.com".to_string(),
            "kimi.moonshot.cn".to_string(),
            "grok.com".to_string(),
            "x.ai".to_string(),
            "copilot.microsoft.com".to_string(),
            "console.anthropic.com".to_string(),
            "aistudio.google.com".to_string(),
        ],
    );
    watch.insert(
        "design".to_string(),
        vec![
            "figma.com".to_string(),
            "sketch.com".to_string(),
            "framer.com".to_string(),
            "behance.net".to_string(),
            "dribbble.com".to_string(),
            "midjourney.com".to_string(),
            "unsplash.com".to_string(),
            "pexels.com".to_string(),
            "excalidraw.com".to_string(),
            "tldraw.com".to_string(),
            "app.diagrams.net".to_string(),
        ],
    );
    watch.insert(
        "productivity".to_string(),
        vec![
            "notion.so".to_string(),
            "linear.app".to_string(),
            "atlassian.net".to_string(),
            "jira.atlassian.com".to_string(),
            "confluence.atlassian.com".to_string(),
            "monday.com".to_string(),
            "airtable.com".to_string(),
            "trello.com".to_string(),
            "asana.com".to_string(),
            "clickup.com".to_string(),
            "shortcut.com".to_string(),
            "height.app".to_string(),
            "todoist.com".to_string(),
        ],
    );
    watch.insert(
        "chat".to_string(),
        vec![
            "discord.com".to_string(),
            "slack.com".to_string(),
            "teams.microsoft.com".to_string(),
            "web.whatsapp.com".to_string(),
            "web.telegram.org".to_string(),
            "messenger.com".to_string(),
            "signal.org".to_string(),
            "element.io".to_string(),
            "app.element.io".to_string(),
        ],
    );
    watch.insert(
        "meeting".to_string(),
        vec![
            "meet.google.com".to_string(),
            "zoom.us".to_string(),
            "app.zoom.us".to_string(),
            "whereby.com".to_string(),
            "around.co".to_string(),
        ],
    );
    watch.insert(
        "video".to_string(),
        vec![
            "youtube.com".to_string(),
            "youtu.be".to_string(),
            "m.youtube.com".to_string(),
            "vimeo.com".to_string(),
            "netflix.com".to_string(),
            "twitch.tv".to_string(),
            "primevideo.com".to_string(),
            "hulu.com".to_string(),
            "disneyplus.com".to_string(),
            "hbomax.com".to_string(),
            "max.com".to_string(),
            "peacocktv.com".to_string(),
            "paramount.plus".to_string(),
            "kick.com".to_string(),
            "dailymotion.com".to_string(),
        ],
    );
    watch.insert(
        "scroll".to_string(),
        vec![
            "reddit.com".to_string(),
            "old.reddit.com".to_string(),
            "x.com".to_string(),
            "twitter.com".to_string(),
            "tiktok.com".to_string(),
            "instagram.com".to_string(),
            "facebook.com".to_string(),
            "threads.net".to_string(),
            "threads.com".to_string(),
            "bsky.app".to_string(),
            "tumblr.com".to_string(),
            "9gag.com".to_string(),
            "news.ycombinator.com".to_string(),
            "lemmy.world".to_string(),
            "kbin.social".to_string(),
        ],
    );
    watch
}

impl Default for DomainsConfig {
    fn default() -> Self {
        default_domains_config()
    }
}

fn default_browsers() -> BTreeMap<String, BrowserConfig> {
    let mut browsers = BTreeMap::new();
    browsers.insert(
        "helium".to_string(),
        BrowserConfig {
            app_ids: vec!["helium".to_string(), "net.imput.helium".to_string()],
            history_paths: vec![
                "~/.config/net.imput.helium/*/History".to_string(),
                "~/.var/app/net.imput.helium/config/net.imput.helium/*/History".to_string(),
            ],
            kind: "chromium".to_string(),
        },
    );
    browsers.insert(
        "brave".to_string(),
        BrowserConfig {
            app_ids: vec![
                "brave-browser".to_string(),
                "brave".to_string(),
                "com.brave.Browser".to_string(),
            ],
            history_paths: vec![
                "~/.config/BraveSoftware/Brave-Browser/*/History".to_string(),
                "~/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser/*/History".to_string(),
                "~/snap/brave/current/.config/BraveSoftware/Brave-Browser/*/History".to_string(),
            ],
            kind: "chromium".to_string(),
        },
    );
    browsers.insert(
        "chrome".to_string(),
        BrowserConfig {
            app_ids: vec![
                "google-chrome".to_string(),
                "chrome".to_string(),
                "com.google.Chrome".to_string(),
            ],
            history_paths: vec![
                "~/.config/google-chrome/*/History".to_string(),
                "~/.var/app/com.google.Chrome/config/google-chrome/*/History".to_string(),
            ],
            kind: "chromium".to_string(),
        },
    );
    browsers.insert(
        "chromium".to_string(),
        BrowserConfig {
            app_ids: vec![
                "chromium".to_string(),
                "chromium-browser".to_string(),
                "org.chromium.Chromium".to_string(),
                "Chromium-browser".to_string(),
            ],
            history_paths: vec![
                "~/.config/chromium/*/History".to_string(),
                "~/.var/app/org.chromium.Chromium/config/chromium/*/History".to_string(),
                "~/snap/chromium/common/chromium/*/History".to_string(),
            ],
            kind: "chromium".to_string(),
        },
    );
    browsers
}

fn default_state_path() -> PathBuf {
    expand_path(DEFAULT_STATE_PATH)
}

fn default_socket_path() -> PathBuf {
    expand_path(DEFAULT_SOCKET_PATH)
}

impl Default for Config {
    fn default() -> Self {
        bundled_default_config()
            .expect("bundled default config must parse")
            .normalized()
    }
}

impl Config {
    fn load_default() -> Result<Self> {
        Self::load_from_path(&expand_path(DEFAULT_CONFIG_PATH))
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        let config = if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            let user_config = toml::from_str::<Config>(&raw)
                .with_context(|| format!("failed to parse config {}", path.display()))?;
            let mut config = bundled_default_config()?;
            config.apply_user_config(user_config);
            config
        } else {
            Config::default()
        };
        let config = config.normalized();
        config.validate()?;
        Ok(config)
    }

    fn normalized(mut self) -> Self {
        self.socket_path = expand_path(self.socket_path.to_string_lossy().as_ref());
        self.state_path = expand_path(self.state_path.to_string_lossy().as_ref());
        for browser in self.browsers.values_mut() {
            browser.app_ids = browser
                .app_ids
                .iter()
                .map(|id| normalize_id(id))
                .collect::<Vec<_>>();
        }
        self
    }

    fn apply_user_config(&mut self, user: Config) {
        self.poll_interval_secs = user.poll_interval_secs;
        self.idle_after_secs = user.idle_after_secs;
        self.socket_path = user.socket_path;
        self.state_path = user.state_path;
        self.display = user.display;
        self.budgets = user.budgets;
        self.breaks = user.breaks;
        self.focus_source = user.focus_source;

        for (name, browser) in user.browsers {
            self.browsers.insert(name, browser);
        }

        self.terminals.poll_secs = user.terminals.poll_secs;
        merge_string_lists(&mut self.terminals.apps, user.terminals.apps);
        merge_string_lists(&mut self.apps.watch, user.apps.watch);
        merge_string_lists(&mut self.domains.watch, user.domains.watch);
    }

    fn validate(&self) -> Result<()> {
        if self.poll_interval_secs == 0 {
            bail!("poll_interval_secs must be greater than 0");
        }
        if self.idle_after_secs <= 0 {
            bail!("idle_after_secs must be greater than 0");
        }
        Ok(())
    }

    fn category_for_app(&self, app_id: &str) -> Option<String> {
        let normalized = normalize_id(app_id);
        let watch_hit = self.apps.watch.iter().find_map(|(category, ids)| {
            ids.iter()
                .any(|candidate| normalize_id(candidate) == normalized)
                .then(|| category.clone())
        });
        if watch_hit.is_some() {
            return watch_hit;
        }
        self.terminals.apps.iter().find_map(|(category, names)| {
            names
                .iter()
                .any(|candidate| normalize_id(candidate) == normalized)
                .then(|| category.clone())
        })
    }

    fn is_terminal_app(&self, app_id: &str) -> bool {
        let normalized = normalize_id(app_id);
        self.apps
            .watch
            .get("terminal")
            .map(|ids| ids.iter().any(|c| normalize_id(c) == normalized))
            .unwrap_or(false)
    }

    fn terminal_subprocess_program(&self, name: &str) -> Option<String> {
        let normalized = normalize_id(name);
        for names in self.terminals.apps.values() {
            for candidate in names {
                if normalize_id(candidate) == normalized {
                    return Some(normalize_id(candidate));
                }
            }
        }
        None
    }

    fn category_for_domain(&self, domain: &str) -> Option<String> {
        let domain = normalize_domain(domain);
        self.domains.watch.iter().find_map(|(category, domains)| {
            domains
                .iter()
                .any(|candidate| domain_matches(&domain, candidate))
                .then(|| category.clone())
        })
    }

    fn browser_app_ids(&self) -> HashSet<String> {
        self.browsers
            .values()
            .flat_map(|browser| browser.app_ids.iter().cloned())
            .collect()
    }
}

fn bundled_default_config() -> Result<Config> {
    toml::from_str::<Config>(DEFAULT_CONFIG_TOML).context("failed to parse bundled default config")
}

fn merge_string_lists(
    base: &mut BTreeMap<String, Vec<String>>,
    overlay: BTreeMap<String, Vec<String>>,
) {
    for (category, values) in overlay {
        let entry = base.entry(category).or_default();
        for value in values {
            if !entry.iter().any(|existing| existing == &value) {
                entry.push(value);
            }
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<Mutex<Config>>,
    db: Arc<Mutex<Connection>>,
    db_state_path: Arc<Mutex<PathBuf>>,
    socket_path: Arc<Mutex<PathBuf>>,
    last_rebuild: Arc<Mutex<Option<Instant>>>,
    tracking_state: Arc<Mutex<Option<PauseInfo>>>,
    /// Resolved focus source kind (e.g. "niri", "hyprland", "sway", "river").
    /// Set once at daemon startup and never changes.
    focus_source_kind: Arc<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseReason {
    Manual,
    Idle,
}

impl PauseReason {
    fn as_str(&self) -> &'static str {
        match self {
            PauseReason::Manual => "manual",
            PauseReason::Idle => "idle",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(PauseReason::Manual),
            "idle" => Some(PauseReason::Idle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PauseInfo {
    at: DateTime<Utc>,
    reason: PauseReason,
}

fn load_pause_state(db: &Connection) -> Result<Option<PauseInfo>> {
    let at = match meta_get(db, "paused_at")? {
        Some(v) => parse_rfc3339_utc(&v)?,
        None => return Ok(None),
    };
    let reason = meta_get(db, "paused_reason")?
        .and_then(|v| PauseReason::from_str(&v))
        .unwrap_or(PauseReason::Manual);
    Ok(Some(PauseInfo { at, reason }))
}

fn persist_pause_state(db: &Connection, info: Option<PauseInfo>) -> Result<()> {
    match info {
        Some(p) => {
            meta_set(db, "paused_at", &p.at.to_rfc3339())?;
            meta_set(db, "paused_reason", p.reason.as_str())?;
        }
        None => {
            meta_delete(db, "paused_at")?;
            meta_delete(db, "paused_reason")?;
        }
    }
    Ok(())
}

const REBUILD_MIN_INTERVAL: StdDuration = StdDuration::from_secs(3);
const HEARTBEAT_INTERVAL: StdDuration = StdDuration::from_secs(30);
const HEARTBEAT_STALE_AFTER: StdDuration = StdDuration::from_secs(60);

fn run_daemon() -> Result<()> {
    cleanup_stale_snapshots();
    let mut config = Config::load_default()?;
    let db = open_state_db(&config.state_path)?;
    let now = Utc::now();
    cap_open_intervals_at_stale_heartbeat(&db, now)?;
    close_stale_open_intervals(&db, now, config.idle_after_secs)?;
    load_breaks_override(&db, &mut config.breaks)?;

    // Resolve focus source kind once at startup.
    let resolved_kind = if config.focus_source.kind == "auto" {
        focus::auto_detect().to_string()
    } else {
        config.focus_source.kind.clone()
    };

    let initial_pause = load_pause_state(&db)?;
    let state = AppState {
        config: Arc::new(Mutex::new(config.clone())),
        db: Arc::new(Mutex::new(db)),
        db_state_path: Arc::new(Mutex::new(config.state_path.clone())),
        socket_path: Arc::new(Mutex::new(config.socket_path.clone())),
        last_rebuild: Arc::new(Mutex::new(None)),
        tracking_state: Arc::new(Mutex::new(initial_pause)),
        focus_source_kind: Arc::new(resolved_kind),
    };
    install_shutdown_handler(state.clone())?;
    let listener = bind_socket(&config.socket_path)?;

    let socket_state = state.clone();
    thread::spawn(move || {
        if let Err(error) = run_socket_server(socket_state, listener) {
            eprintln!("attn daemon socket error: {error:#}");
        }
    });

    let import_state = state.clone();
    thread::spawn(move || loop {
        let start = Instant::now();
        if let Err(error) = import_domains_for_today(&import_state) {
            eprintln!("attn browser import error: {error:#}");
        }
        let poll_secs = import_state
            .config
            .lock()
            .map(|config| config.poll_interval_secs)
            .unwrap_or_else(|_| default_poll_interval_secs());
        let elapsed = start.elapsed();
        if elapsed < StdDuration::from_secs(poll_secs) {
            thread::sleep(StdDuration::from_secs(poll_secs) - elapsed);
        }
    });

    let terminal_state = state.clone();
    thread::spawn(move || loop {
        let poll_secs = terminal_state
            .config
            .lock()
            .map(|config| config.terminals.poll_secs.max(1))
            .unwrap_or(default_terminal_poll_secs());
        thread::sleep(StdDuration::from_secs(poll_secs));
        let window = query_focused_window(&terminal_state).ok().flatten();
        if let Err(error) = handle_focus_change_with(&terminal_state, window) {
            eprintln!("attn terminal poll error: {error:#}");
        }
    });

    let heartbeat_state = state.clone();
    thread::spawn(move || loop {
        {
            let db = heartbeat_state.db.lock().unwrap();
            if let Err(error) = meta_set(&db, "last_heartbeat_at", &Utc::now().to_rfc3339()) {
                eprintln!("attn heartbeat error: {error:#}");
            }
        }
        thread::sleep(HEARTBEAT_INTERVAL);
    });

    let config_watch_state = state.clone();
    thread::spawn(move || run_config_watcher(config_watch_state));

    let dbus_state = state.clone();
    thread::spawn(move || loop {
        if let Err(error) = run_dbus_sleep_listener(&dbus_state) {
            eprintln!("attn dbus sleep listener error: {error:#}");
            thread::sleep(StdDuration::from_secs(10));
        }
    });

    let wayland_state = state.clone();
    thread::spawn(move || loop {
        if let Err(error) = run_wayland_idle_listener(&wayland_state) {
            eprintln!("attn wayland idle listener error: {error:#}");
        }
        thread::sleep(StdDuration::from_secs(5));
    });

    run_focus_dispatch(state)
}

fn run_wayland_idle_listener(state: &AppState) -> Result<()> {
    use wayland_client::Connection as WConnection;

    let timeout_ms: u32 = state
        .config
        .lock()
        .map(|c| (c.breaks.min_break_secs.max(1) as u32).saturating_mul(1000))
        .unwrap_or(300_000);

    let conn = WConnection::connect_to_env().context("wayland connect")?;
    let mut event_queue = conn.new_event_queue::<IdleClient>();
    let qh = event_queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());

    let mut client = IdleClient {
        state: state.clone(),
        seat: None,
        notifier: None,
        notification: None,
        timeout_ms,
        attempted: false,
    };

    // First roundtrip surfaces the globals.
    event_queue.roundtrip(&mut client).context("wayland roundtrip")?;
    // Second roundtrip pulls follow-up events (e.g. wl_seat capabilities).
    event_queue.roundtrip(&mut client).context("wayland roundtrip 2")?;

    client.try_subscribe(&qh)?;

    loop {
        event_queue
            .blocking_dispatch(&mut client)
            .context("wayland dispatch")?;
    }
}

struct IdleClient {
    state: AppState,
    seat: Option<wayland_client::protocol::wl_seat::WlSeat>,
    notifier: Option<wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1>,
    notification: Option<wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1>,
    timeout_ms: u32,
    attempted: bool,
}

impl IdleClient {
    fn try_subscribe(&mut self, qh: &wayland_client::QueueHandle<Self>) -> Result<()> {
        if self.notification.is_some() || self.attempted {
            return Ok(());
        }
        let (Some(seat), Some(notifier)) = (self.seat.as_ref(), self.notifier.as_ref()) else {
            return Ok(());
        };
        let notification = notifier.get_idle_notification(self.timeout_ms, seat, qh, ());
        self.notification = Some(notification);
        self.attempted = true;
        Ok(())
    }

    fn on_idled(&self) {
        let now = Utc::now();
        let mut current = self.state.tracking_state.lock().unwrap();
        if current.is_some() {
            return;
        }
        let info = PauseInfo { at: now, reason: PauseReason::Idle };
        let config = self.state.config.lock().unwrap().clone();
        let db = self.state.db.lock().unwrap();
        if let Err(e) = close_open_interval(&db, now, config.idle_after_secs) {
            eprintln!("attn idle: close interval failed: {e:#}");
        }
        if let Err(e) = persist_pause_state(&db, Some(info)) {
            eprintln!("attn idle: persist pause failed: {e:#}");
        }
        *current = Some(info);
    }

    fn on_resumed(&self) {
        let mut current = self.state.tracking_state.lock().unwrap();
        let Some(info) = current.as_ref().copied() else {
            return;
        };
        if info.reason != PauseReason::Idle {
            return;
        }
        let now = Utc::now();
        let db = self.state.db.lock().unwrap();
        if let Err(e) = persist_pause_state(&db, None) {
            eprintln!("attn idle: persist clear failed: {e:#}");
        }
        if let Err(e) = meta_set(&db, "session_reset_at", &now.to_rfc3339()) {
            eprintln!("attn idle: persist session_reset failed: {e:#}");
        }
        *current = None;
        drop(db);
        if let Ok(Some(window)) = query_focused_window(&self.state) {
            let config = self.state.config.lock().unwrap().clone();
            let resolved = resolve_focused_window(window, &config);
            let db = self.state.db.lock().unwrap();
            if let Err(e) = open_app_interval(&db, now, &resolved) {
                eprintln!("attn idle: reopen interval failed: {e:#}");
            }
        }
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()> for IdleClient {
    fn event(
        client: &mut Self,
        registry: &wayland_client::protocol::wl_registry::WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => {
                    let seat: wayland_client::protocol::wl_seat::WlSeat =
                        registry.bind(name, version.min(7), qh, ());
                    client.seat = Some(seat);
                }
                "ext_idle_notifier_v1" => {
                    let notifier: wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1 =
                        registry.bind(name, version.min(2), qh, ());
                    client.notifier = Some(notifier);
                }
                _ => {}
            }
        }
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_seat::WlSeat, ()> for IdleClient {
    fn event(
        _client: &mut Self,
        _seat: &wayland_client::protocol::wl_seat::WlSeat,
        _event: wayland_client::protocol::wl_seat::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

impl
    wayland_client::Dispatch<
        wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1,
        (),
    > for IdleClient
{
    fn event(
        _client: &mut Self,
        _notifier: &wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1,
        _event: wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

impl
    wayland_client::Dispatch<
        wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1,
        (),
    > for IdleClient
{
    fn event(
        client: &mut Self,
        _notification: &wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1,
        event: wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::Event;
        match event {
            Event::Idled => client.on_idled(),
            Event::Resumed => client.on_resumed(),
            _ => {}
        }
    }
}

fn run_dbus_sleep_listener(state: &AppState) -> Result<()> {
    use zbus::blocking::Connection as ZConnection;
    use zbus::blocking::MessageIterator;
    use zbus::MatchRule;

    let conn = ZConnection::system().context("connect to system D-Bus")?;
    let rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .interface("org.freedesktop.login1.Manager")
        .context("set match interface")?
        .member("PrepareForSleep")
        .context("set match member")?
        .build();
    zbus::blocking::fdo::DBusProxy::new(&conn)
        .context("create DBus proxy")?
        .add_match_rule(rule)
        .context("install PrepareForSleep match")?;

    let iter = MessageIterator::from(&conn);
    for msg in iter {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                // Don't spin: bail to the outer thread loop which sleeps then reconnects.
                return Err(anyhow::anyhow!("dbus iter error: {e:#}"));
            }
        };
        let going_to_sleep: bool = match msg.body().deserialize() {
            Ok(b) => b,
            Err(_) => continue,
        };
        if going_to_sleep {
            let now = Utc::now();
            let config = state.config.lock().unwrap().clone();
            let db = state.db.lock().unwrap();
            let _ = close_open_interval(&db, now, config.idle_after_secs);
        }
    }
    Ok(())
}

fn run_config_watcher(state: AppState) {
    let path = expand_path(DEFAULT_CONFIG_PATH);
    let mut last_mtime: Option<std::time::SystemTime> = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok();
    loop {
        thread::sleep(StdDuration::from_secs(3));
        let mtime = match fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(m) => Some(m),
            Err(_) => None,
        };
        if mtime != last_mtime {
            last_mtime = mtime;
            if mtime.is_none() {
                continue;
            }
            if let Err(error) = reload_daemon_config(&state) {
                eprintln!("attn config auto-reload error: {error:#}");
            }
        }
    }
}

fn cap_open_intervals_at_stale_heartbeat(db: &Connection, now: DateTime<Utc>) -> Result<()> {
    let Some(value) = meta_get(db, "last_heartbeat_at")? else {
        return Ok(());
    };
    let last = parse_rfc3339_utc(&value)?;
    let age = now.signed_duration_since(last);
    if age.num_seconds() < HEARTBEAT_STALE_AFTER.as_secs() as i64 {
        return Ok(());
    }
    let mut stmt = db.prepare("SELECT id, started_at FROM app_intervals WHERE ended_at IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let open_intervals = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, started_at) in open_intervals {
        let started_at = parse_rfc3339_utc(&started_at)?;
        let ended_at = last.max(started_at);
        db.execute(
            "UPDATE app_intervals SET ended_at = ?1, idle_adjusted = 1 WHERE id = ?2",
            params![ended_at.to_rfc3339(), id],
        )?;
    }
    Ok(())
}

fn install_shutdown_handler(state: AppState) -> Result<()> {
    let shutting_down = Arc::new(AtomicBool::new(false));
    let handler_flag = shutting_down.clone();
    ctrlc::set_handler(move || {
        if handler_flag.swap(true, Ordering::SeqCst) {
            std::process::exit(1);
        }

        let now = Utc::now();
        let config = state.config.lock().ok().map(|config| config.clone());
        if let Some(config) = config {
            if let Ok(db) = state.db.lock() {
                let _ = close_open_interval(&db, now, config.idle_after_secs);
            }
            if let Ok(socket_path) = state.socket_path.lock() {
                let _ = fs::remove_file(&*socket_path);
            }
        }

        std::process::exit(0);
    })
    .context("failed to install shutdown handler")
}

fn open_state_db(path: &Path) -> Result<Connection> {
    ensure_parent_dir(path)?;
    let db = Connection::open(path)
        .with_context(|| format!("failed to open state DB {}", path.display()))?;
    migrate(&db)?;
    harden_sqlite_permissions(path)?;
    Ok(db)
}

fn bind_socket(socket_path: &Path) -> Result<UnixListener> {
    ensure_parent_dir(socket_path)?;
    cleanup_socket_path(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind socket {}", socket_path.display()))?;
    harden_file_permissions(socket_path)?;
    Ok(listener)
}

fn run_socket_server(state: AppState, listener: UnixListener) -> Result<()> {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let client_state = state.clone();
                thread::spawn(move || {
                    if let Err(error) = handle_client(client_state, stream) {
                        eprintln!("attn client error: {error:#}");
                    }
                });
            }
            Err(error) => eprintln!("attn socket accept error: {error}"),
        }
    }

    Ok(())
}

fn cleanup_socket_path(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }
    match UnixStream::connect(socket_path) {
        Ok(_) => bail!(
            "socket {} is already in use; is attn already running?",
            socket_path.display()
        ),
        Err(_) => fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display())),
    }
}

fn handle_client(state: AppState, mut stream: UnixStream) -> Result<()> {
    let mut request = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut request)?;
    }
    let request = request.trim();
    if request.is_empty() {
        return Ok(());
    }
    let response = match request {
        "status" => {
            let config = state.config.lock().unwrap().clone();
            let db = state.db.lock().unwrap();
            let mut last = state.last_rebuild.lock().unwrap();
            let should_rebuild = last
                .map(|t| t.elapsed() >= REBUILD_MIN_INTERVAL)
                .unwrap_or(true);
            let status = build_status(&db, &config, should_rebuild)?;
            if should_rebuild {
                *last = Some(Instant::now());
            }
            serde_json::to_string(&status)?
        }
        "reload" => {
            let response = reload_daemon_config(&state)?;
            serde_json::to_string(&response)?
        }
        "break_start" => {
            let response = set_pause(&state, PauseReason::Manual)?;
            serde_json::to_string(&response)?
        }
        "break_end" => {
            let response = clear_pause(&state)?;
            serde_json::to_string(&response)?
        }
        cmd if cmd.starts_with("set_breaks ") => {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.len() != 4 {
                serde_json::to_string(&serde_json::json!({
                    "ok": false,
                    "error": "set_breaks expects: <enabled:0|1> <interval_secs> <min_break_secs>"
                }))?
            } else {
                let enabled = parts[1] == "1" || parts[1] == "true";
                let interval = parts[2].parse::<i64>().unwrap_or(3600).max(60);
                let min_break = parts[3].parse::<i64>().unwrap_or(300).max(30);
                apply_breaks_override(&state, enabled, interval, min_break)?;
                serde_json::to_string(&serde_json::json!({
                    "ok": true,
                    "enabled": enabled,
                    "interval_secs": interval,
                    "min_break_secs": min_break,
                }))?
            }
        }
        other => serde_json::to_string(&serde_json::json!({
            "ok": false,
            "error": format!("unknown request: {other}")
        }))?,
    };
    stream.write_all(response.as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(())
}

fn socket_request(socket_path: &Path, request: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(SOCKET_REQUEST_TIMEOUT))
        .context("failed to set socket read timeout")?;
    stream
        .set_write_timeout(Some(SOCKET_REQUEST_TIMEOUT))
        .context("failed to set socket write timeout")?;
    stream.write_all(request.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response.trim().to_string())
}

#[derive(Debug, Serialize)]
struct ReloadResponse {
    ok: bool,
    state_reopened: bool,
    socket_restart_required: bool,
}

#[derive(Debug, Serialize)]
struct PauseResponse {
    ok: bool,
    paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    paused_reason: Option<String>,
}

fn apply_breaks_override(
    state: &AppState,
    enabled: bool,
    interval_secs: i64,
    min_break_secs: i64,
) -> Result<()> {
    let db = state.db.lock().unwrap();
    meta_set(&db, "breaks_enabled", if enabled { "1" } else { "0" })?;
    meta_set(&db, "breaks_interval_secs", &interval_secs.to_string())?;
    meta_set(&db, "breaks_min_break_secs", &min_break_secs.to_string())?;
    drop(db);
    let mut config = state.config.lock().unwrap();
    config.breaks.enabled = enabled;
    config.breaks.interval_secs = interval_secs;
    config.breaks.min_break_secs = min_break_secs;
    Ok(())
}

fn load_breaks_override(db: &Connection, breaks: &mut BreaksConfig) -> Result<()> {
    if let Some(v) = meta_get(db, "breaks_enabled")? {
        breaks.enabled = v == "1" || v == "true";
    }
    if let Some(v) = meta_get(db, "breaks_interval_secs")? {
        if let Ok(parsed) = v.parse::<i64>() {
            breaks.interval_secs = parsed.max(60);
        }
    }
    if let Some(v) = meta_get(db, "breaks_min_break_secs")? {
        if let Ok(parsed) = v.parse::<i64>() {
            breaks.min_break_secs = parsed.max(30);
        }
    }
    Ok(())
}

fn set_pause(state: &AppState, reason: PauseReason) -> Result<PauseResponse> {
    let now = Utc::now();
    let mut current = state.tracking_state.lock().unwrap();
    if let Some(info) = current.as_mut() {
        if info.reason != reason {
            info.reason = reason;
            let db = state.db.lock().unwrap();
            persist_pause_state(&db, Some(*info))?;
        }
        return Ok(PauseResponse {
            ok: true,
            paused: true,
            paused_reason: Some(info.reason.as_str().to_string()),
        });
    }
    let info = PauseInfo { at: now, reason };
    let config = state.config.lock().unwrap().clone();
    let db = state.db.lock().unwrap();
    close_open_interval(&db, now, config.idle_after_secs)?;
    persist_pause_state(&db, Some(info))?;
    *current = Some(info);
    Ok(PauseResponse {
        ok: true,
        paused: true,
        paused_reason: Some(reason.as_str().to_string()),
    })
}

fn clear_pause(state: &AppState) -> Result<PauseResponse> {
    let mut current = state.tracking_state.lock().unwrap();
    if current.is_none() {
        return Ok(PauseResponse {
            ok: true,
            paused: false,
            paused_reason: None,
        });
    }
    let now = Utc::now();
    let db = state.db.lock().unwrap();
    persist_pause_state(&db, None)?;
    meta_set(&db, "session_reset_at", &now.to_rfc3339())?;
    *current = None;
    drop(db);
    if let Ok(Some(window)) = query_focused_window(state) {
        let config = state.config.lock().unwrap().clone();
        let resolved = resolve_focused_window(window, &config);
        let db = state.db.lock().unwrap();
        open_app_interval(&db, now, &resolved)?;
    }
    Ok(PauseResponse {
        ok: true,
        paused: false,
        paused_reason: None,
    })
}

fn reload_daemon_config(state: &AppState) -> Result<ReloadResponse> {
    let mut new_config = Config::load_default()?;
    let old_config = state.config.lock().unwrap().clone();
    let state_reopened = old_config.state_path != new_config.state_path;
    let socket_restart_required = old_config.socket_path != new_config.socket_path;

    if state_reopened {
        let now = Utc::now();
        swap_state_db_for_reload(state, &old_config, &new_config, now)?;
        if let Ok(Some(window)) = query_focused_window(state) {
            let db = state.db.lock().unwrap();
            open_app_interval(&db, Utc::now(), &window)?;
        }
    }

    {
        let db = state.db.lock().unwrap();
        load_breaks_override(&db, &mut new_config.breaks)?;
    }

    let mut config = state.config.lock().unwrap();
    *config = new_config;

    Ok(ReloadResponse {
        ok: true,
        state_reopened,
        socket_restart_required,
    })
}

fn swap_state_db_for_reload(
    state: &AppState,
    old_config: &Config,
    new_config: &Config,
    now: DateTime<Utc>,
) -> Result<()> {
    let new_db = open_state_db(&new_config.state_path)?;
    close_stale_open_intervals(&new_db, now, new_config.idle_after_secs)?;
    let mut db = state.db.lock().unwrap();
    close_open_interval(&db, now, old_config.idle_after_secs)?;
    *db = new_db;
    *state.db_state_path.lock().unwrap() = new_config.state_path.clone();
    Ok(())
}

/// Dispatch the correct focus adapter based on `state.focus_source_kind`,
/// receive events on a channel, and update the DB on each event.
fn run_focus_dispatch(state: AppState) -> Result<()> {
    let kind = state.focus_source_kind.as_str();
    let source = focus::build(kind)?;
    eprintln!("attn focus source: {}", source.name());

    // Open an interval for the current window at startup.
    let paused = state.tracking_state.lock().unwrap().is_some();
    if !paused {
        if let Some(window) = source.poll_current()? {
            let config = state.config.lock().unwrap().clone();
            let resolved = resolve_focused_window(window, &config);
            open_app_interval(&state.db.lock().unwrap(), Utc::now(), &resolved)?;
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<focus::FocusEvent>();

    thread::spawn(move || {
        if let Err(error) = source.run(tx) {
            eprintln!("attn focus source exited: {error:#}");
        }
    });

    for event in rx {
        let window = match event {
            focus::FocusEvent::Focused(w) => Some(w),
            focus::FocusEvent::Unfocused => None,
        };
        if let Err(error) = handle_focus_change_with(&state, window) {
            eprintln!("attn focus change error: {error:#}");
        }
    }

    Ok(())
}

fn handle_focus_change_with(state: &AppState, current_window: Option<focus::FocusedWindow>) -> Result<()> {
    let now = Utc::now();
    let paused = state.tracking_state.lock().unwrap().is_some();
    let config = state.config.lock().unwrap().clone();
    let db = state.db.lock().unwrap();
    let open = open_app_identity(&db)?;

    if paused {
        if open.is_some() {
            close_open_interval(&db, now, config.idle_after_secs)?;
        }
        return Ok(());
    }

    let current = current_window.map(|w| resolve_focused_window(w, &config));
    match (open, current) {
        (Some(open), Some(window)) if open.matches(&window) => {}
        (Some(_), Some(window)) => {
            close_open_interval(&db, now, config.idle_after_secs)?;
            open_app_interval(&db, now, &window)?;
        }
        (Some(_), None) => {
            close_open_interval(&db, now, config.idle_after_secs)?;
        }
        (None, Some(window)) => {
            open_app_interval(&db, now, &window)?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// Poll the currently focused window via the stored focus source kind.
fn query_focused_window(state: &AppState) -> Result<Option<FocusedWindow>> {
    let kind = state.focus_source_kind.as_str();
    let source = focus::build(kind)?;
    source.poll_current()
}

/// Re-exported from focus module for local use.
use focus::FocusedWindow;

#[derive(Clone, Debug)]
struct OpenAppInterval {
    window_id: Option<i64>,
    app_id: String,
}

impl OpenAppInterval {
    fn matches(&self, window: &FocusedWindow) -> bool {
        if self.app_id != window.app_id {
            return false;
        }
        if let (Some(open_window_id), Some(current_window_id)) = (self.window_id, window.window_id)
        {
            return open_window_id == current_window_id;
        }
        true
    }
}

fn resolve_focused_window(mut window: FocusedWindow, config: &Config) -> FocusedWindow {
    if !config.is_terminal_app(&window.app_id) {
        return window;
    }
    if let Some(name) = match_terminal_program_in_title(&window.title, config) {
        window.app_id = name;
        return window;
    }
    let Some(pid) = window.pid else { return window; };
    let descendants = collect_proc_descendants(pid);
    let descendant_pids: HashSet<i32> = descendants.iter().map(|d| d.pid).collect();
    if let Some(name) = pick_tmux_subprocess(&descendant_pids, config) {
        window.app_id = name;
        return window;
    }
    if let Some(name) = pick_terminal_subprocess(&descendants, config) {
        window.app_id = name;
    }
    window
}

fn pick_tmux_subprocess(descendant_pids: &HashSet<i32>, config: &Config) -> Option<String> {
    if descendant_pids.is_empty() {
        return None;
    }
    let output = Command::new("tmux")
        .args(["list-clients", "-F", "#{client_pid}|#{client_session}|#{client_activity}"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut best: Option<(u64, String)> = None;
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() < 3 {
            continue;
        }
        let client_pid: i32 = match parts[0].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !descendant_pids.contains(&client_pid) {
            continue;
        }
        let session = parts[1];
        let activity: u64 = parts[2].parse().unwrap_or(0);
        let cmd_output = Command::new("tmux")
            .args([
                "display-message",
                "-p",
                "-t",
                session,
                "#{pane_current_command}",
            ])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !cmd_output.status.success() {
            continue;
        }
        let cmd = String::from_utf8_lossy(&cmd_output.stdout).trim().to_string();
        if let Some(name) = config.terminal_subprocess_program(&cmd) {
            if best.as_ref().map(|(act, _)| activity > *act).unwrap_or(true) {
                best = Some((activity, name));
            }
        }
    }
    best.map(|(_, name)| name)
}

fn match_terminal_program_in_title(title: &str, config: &Config) -> Option<String> {
    if title.is_empty() {
        return None;
    }
    let lowered = title.to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for names in config.terminals.apps.values() {
        for candidate in names {
            let normalized = normalize_id(candidate);
            if normalized.is_empty() {
                continue;
            }
            if let Some(idx) = lowered.find(&normalized) {
                let ok_left = idx == 0 || !is_word_char(lowered.as_bytes()[idx - 1]);
                let end = idx + normalized.len();
                let ok_right = end == lowered.len() || !is_word_char(lowered.as_bytes()[end]);
                if ok_left && ok_right {
                    if best.as_ref().map(|(prev, _)| idx < *prev).unwrap_or(true) {
                        best = Some((idx, normalized));
                    }
                }
            }
        }
    }
    best.map(|(_, name)| name)
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[derive(Debug)]
struct DescendantProc {
    pid: i32,
    depth: i32,
    name: String,
    start_time: u64,
}

fn pick_terminal_subprocess(
    candidates: &[DescendantProc],
    config: &Config,
) -> Option<String> {
    candidates
        .iter()
        .filter_map(|p| {
            config
                .terminal_subprocess_program(&p.name)
                .map(|matched| (p.start_time, p.depth, matched))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, name)| name)
}

fn collect_proc_descendants(root_pid: i32) -> Vec<DescendantProc> {
    let mut out: Vec<DescendantProc> = Vec::new();
    let mut seen: HashSet<i32> = HashSet::new();
    let mut stack: Vec<(i32, i32)> = vec![(root_pid, 0)];
    while let Some((pid, depth)) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if depth > 0 {
            if let Some(name) = read_proc_program_name(pid) {
                let start_time = read_proc_start_time(pid).unwrap_or(0);
                out.push(DescendantProc { pid, depth, name, start_time });
            }
        }
        for child in read_proc_children(pid) {
            stack.push((child, depth + 1));
        }
    }
    out
}

fn read_proc_start_time(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let rest = &stat[close + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    fields.get(19).and_then(|v| v.parse::<u64>().ok())
}

fn read_proc_children(pid: i32) -> Vec<i32> {
    let mut result = Vec::new();
    let task_dir = format!("/proc/{}/task", pid);
    let entries = match fs::read_dir(&task_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let path = entry.path().join("children");
        let Ok(data) = fs::read_to_string(&path) else { continue; };
        for token in data.split_whitespace() {
            if let Ok(child) = token.parse::<i32>() {
                result.push(child);
            }
        }
    }
    result
}

fn read_proc_program_name(pid: i32) -> Option<String> {
    if let Ok(data) = fs::read(format!("/proc/{}/cmdline", pid)) {
        let argv0 = data
            .split(|b| *b == 0)
            .find(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned());
        if let Some(arg) = argv0 {
            let basename = Path::new(&arg)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(arg);
            if !basename.is_empty() {
                return Some(basename);
            }
        }
    }
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS app_intervals (
          id INTEGER PRIMARY KEY,
          started_at TEXT NOT NULL,
          ended_at TEXT,
          window_id INTEGER,
          app_id TEXT NOT NULL,
          window_title TEXT NOT NULL DEFAULT '',
          idle_adjusted INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS domain_intervals (
          id INTEGER PRIMARY KEY,
          started_at TEXT NOT NULL,
          ended_at TEXT NOT NULL,
          browser_name TEXT NOT NULL DEFAULT '',
          browser_app_id TEXT NOT NULL,
          domain TEXT NOT NULL,
          url TEXT NOT NULL DEFAULT '',
          source_profile TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS daily_app_totals (
          day TEXT NOT NULL,
          app_id TEXT NOT NULL,
          seconds INTEGER NOT NULL,
          watch_category TEXT,
          PRIMARY KEY (day, app_id)
        );

        CREATE TABLE IF NOT EXISTS daily_domain_totals (
          day TEXT NOT NULL,
          domain TEXT NOT NULL,
          seconds INTEGER NOT NULL,
          watch_category TEXT,
          PRIMARY KEY (day, domain)
        );

        CREATE TABLE IF NOT EXISTS meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS app_intervals_started_idx
          ON app_intervals(started_at);
        CREATE INDEX IF NOT EXISTS app_intervals_app_started_idx
          ON app_intervals(app_id, started_at);
        CREATE INDEX IF NOT EXISTS domain_intervals_started_idx
          ON domain_intervals(started_at);
        ",
    )?;
    add_column_if_missing(db, "app_intervals", "window_id", "INTEGER")?;
    add_column_if_missing(
        db,
        "domain_intervals",
        "browser_name",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    Ok(())
}

fn meta_get(db: &Connection, key: &str) -> Result<Option<String>> {
    db.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}

fn meta_set(db: &Connection, key: &str, value: &str) -> Result<()> {
    db.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn meta_delete(db: &Connection, key: &str) -> Result<()> {
    db.execute("DELETE FROM meta WHERE key = ?1", params![key])?;
    Ok(())
}

fn add_column_if_missing(
    db: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if table_column_exists(db, table, column)? {
        return Ok(());
    }
    db.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn table_column_exists(db: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn close_stale_open_intervals(
    db: &Connection,
    now: DateTime<Utc>,
    idle_after_secs: i64,
) -> Result<()> {
    let mut stmt = db.prepare("SELECT id, started_at FROM app_intervals WHERE ended_at IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let stale_intervals = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    for (id, started_at) in stale_intervals {
        let started_at = parse_rfc3339_utc(&started_at)?;
        let ended_at = capped_interval_end(started_at, None, now, idle_after_secs);
        db.execute(
            "UPDATE app_intervals SET ended_at = ?1, idle_adjusted = ?2 WHERE id = ?3",
            params![ended_at.to_rfc3339(), bool_to_int(ended_at < now), id],
        )?;
    }
    Ok(())
}

fn open_app_interval(db: &Connection, now: DateTime<Utc>, window: &FocusedWindow) -> Result<()> {
    let open: Option<i64> = db
        .query_row(
            "SELECT id FROM app_intervals WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if open.is_some() {
        return Ok(());
    }
    db.execute(
        "INSERT INTO app_intervals(started_at, window_id, app_id, window_title)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            now.to_rfc3339(),
            window.window_id,
            window.app_id,
            window.title
        ],
    )?;
    Ok(())
}

fn open_app_identity(db: &Connection) -> Result<Option<OpenAppInterval>> {
    db.query_row(
        "SELECT window_id, app_id FROM app_intervals WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",
        [],
        |row| {
            Ok(OpenAppInterval {
                window_id: row.get(0)?,
                app_id: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn close_open_interval(db: &Connection, now: DateTime<Utc>, idle_after_secs: i64) -> Result<()> {
    let row: Option<(i64, String)> = db
        .query_row(
            "SELECT id, started_at FROM app_intervals WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, started_at)) = row else {
        return Ok(());
    };
    let started_at = parse_rfc3339_utc(&started_at)?;
    let ended_at = capped_interval_end(started_at, None, now, idle_after_secs);
    let idle_adjusted = ended_at < now;
    db.execute(
        "UPDATE app_intervals SET ended_at = ?1, idle_adjusted = ?2 WHERE id = ?3",
        params![ended_at.to_rfc3339(), bool_to_int(idle_adjusted), id],
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct Status {
    date: String,
    updated_at: String,
    watch_seconds: i64,
    tracked_seconds: i64,
    apps: Vec<AppStatus>,
    domains: Vec<DomainStatus>,
    categories: Vec<CategoryStatus>,
    days: Vec<DaySummary>,
    active_session_seconds: i64,
    break_overdue: bool,
    paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    paused_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paused_since: Option<String>,
    breaks_enabled: bool,
    breaks_interval_secs: i64,
    breaks_min_break_secs: i64,
}

#[derive(Debug, Serialize)]
struct DaySummary {
    date: String,
    tracked_seconds: i64,
    watch_seconds: i64,
    categories: Vec<CategoryStatus>,
}

#[derive(Debug, Serialize)]
struct CategoryStatus {
    name: String,
    seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AppStatus {
    id: String,
    seconds: i64,
    watched: bool,
    category: Option<String>,
}

#[derive(Debug, Serialize)]
struct DomainStatus {
    domain: String,
    seconds: i64,
    watched: bool,
    category: Option<String>,
}

fn compute_active_session_seconds(
    db: &Connection,
    now: DateTime<Utc>,
    min_break_secs: i64,
    idle_after_secs: i64,
) -> Result<i64> {
    let session_reset_at = meta_get(db, "session_reset_at")?
        .map(|v| parse_rfc3339_utc(&v))
        .transpose()?;

    let mut stmt = db.prepare(
        "SELECT started_at, ended_at FROM app_intervals
         ORDER BY started_at DESC LIMIT 1000",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut last_start: DateTime<Utc> = now;
    let mut session: i64 = 0;
    for r in rows {
        let (started_at, ended_at) = r?;
        let start = parse_rfc3339_utc(&started_at)?;
        let (raw_end, is_open) = match ended_at {
            Some(s) => (parse_rfc3339_utc(&s)?.min(now), false),
            None => (now, true),
        };
        let raw_end = raw_end.max(start);
        // Keep the live interval moving for break reminders. Closed intervals
        // still use the idle cap so historical stale focus does not bridge
        // across a real break.
        let end = if is_open {
            raw_end
        } else {
            (start + Duration::seconds(idle_after_secs.max(1))).min(raw_end)
        };
        if (last_start - end).num_seconds() >= min_break_secs {
            break;
        }
        if let Some(reset_at) = session_reset_at {
            if end <= reset_at {
                break;
            }
        }
        let effective_start = match session_reset_at {
            Some(reset_at) if start < reset_at => reset_at,
            _ => start,
        };
        let dur = (end - effective_start).num_seconds().max(0);
        session += dur;
        last_start = start;
    }
    Ok(session)
}

fn build_status(db: &Connection, config: &Config, rebuild: bool) -> Result<Status> {
    let now = Utc::now();
    if rebuild {
        rebuild_daily_totals(db, config, now)?;
    }
    let day = Local::now().date_naive();
    let day_string = day.to_string();
    let browser_ids = config.browser_app_ids();

    let apps = load_app_status(db, &day_string)?;
    let domains = load_domain_status(db, &day_string, &config.display)?;

    let watched_non_browser_apps: i64 = apps
        .iter()
        .filter(|app| app.watched && !browser_ids.contains(&normalize_id(&app.id)))
        .map(|app| app.seconds)
        .sum();
    let watched_domains: i64 = domains
        .iter()
        .filter(|domain| domain.watched)
        .map(|domain| domain.seconds)
        .sum();
    let watch_seconds = watched_non_browser_apps + watched_domains;

    let mut category_totals: BTreeMap<String, i64> = BTreeMap::new();
    for app in &apps {
        if let Some(cat) = &app.category {
            if browser_ids.contains(&normalize_id(&app.id)) {
                continue;
            }
            *category_totals.entry(cat.clone()).or_default() += app.seconds;
        }
    }
    for domain in &domains {
        if let Some(cat) = &domain.category {
            *category_totals.entry(cat.clone()).or_default() += domain.seconds;
        }
    }
    for name in config.budgets.keys() {
        category_totals.entry(name.clone()).or_insert(0);
    }

    let mut categories: Vec<CategoryStatus> = category_totals
        .into_iter()
        .map(|(name, seconds)| {
            let budget_secs = config
                .budgets
                .get(&name)
                .and_then(|b| (b.daily_budget_secs > 0).then_some(b.daily_budget_secs));
            CategoryStatus { name, seconds, budget_secs }
        })
        .collect();
    categories.sort_by(|a, b| b.seconds.cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));

    let tracked_seconds: i64 = categories.iter().map(|c| c.seconds).sum();

    let mut days: Vec<DaySummary> = Vec::with_capacity(7);
    for offset in 0..7 {
        let d = day - Duration::days(offset);
        days.push(build_day_summary(db, config, d)?);
    }

    let pause = load_pause_state(db)?;
    let paused = pause.is_some();
    let paused_reason = pause.as_ref().map(|p| p.reason.as_str().to_string());
    let paused_since = pause.as_ref().map(|p| p.at.to_rfc3339());

    let active_session_seconds = if paused {
        0
    } else {
        compute_active_session_seconds(
            db,
            now,
            config.breaks.min_break_secs,
            config.idle_after_secs,
        )?
    };
    let break_overdue = config.breaks.enabled
        && !paused
        && active_session_seconds >= config.breaks.interval_secs;

    Ok(Status {
        date: day_string,
        updated_at: Local::now().to_rfc3339(),
        watch_seconds,
        tracked_seconds,
        apps,
        domains,
        categories,
        days,
        active_session_seconds,
        break_overdue,
        paused,
        paused_reason,
        paused_since,
        breaks_enabled: config.breaks.enabled,
        breaks_interval_secs: config.breaks.interval_secs,
        breaks_min_break_secs: config.breaks.min_break_secs,
    })
}

fn build_day_summary(db: &Connection, config: &Config, day: NaiveDate) -> Result<DaySummary> {
    let day_string = day.to_string();
    let browser_ids = config.browser_app_ids();
    let apps = load_app_status(db, &day_string)?;
    let domains = load_domain_status(db, &day_string, &config.display)?;

    let watched_non_browser_apps: i64 = apps
        .iter()
        .filter(|app| app.watched && !browser_ids.contains(&normalize_id(&app.id)))
        .map(|app| app.seconds)
        .sum();
    let watched_domains: i64 = domains
        .iter()
        .filter(|d| d.watched)
        .map(|d| d.seconds)
        .sum();
    let watch_seconds = watched_non_browser_apps + watched_domains;

    let mut category_totals: BTreeMap<String, i64> = BTreeMap::new();
    for app in &apps {
        if let Some(cat) = &app.category {
            if browser_ids.contains(&normalize_id(&app.id)) {
                continue;
            }
            *category_totals.entry(cat.clone()).or_default() += app.seconds;
        }
    }
    for domain in &domains {
        if let Some(cat) = &domain.category {
            *category_totals.entry(cat.clone()).or_default() += domain.seconds;
        }
    }

    let mut categories: Vec<CategoryStatus> = category_totals
        .into_iter()
        .map(|(name, seconds)| CategoryStatus { name, seconds, budget_secs: None })
        .collect();
    categories.sort_by(|a, b| b.seconds.cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));
    let tracked_seconds: i64 = categories.iter().map(|c| c.seconds).sum();

    Ok(DaySummary { date: day_string, tracked_seconds, watch_seconds, categories })
}

fn rebuild_daily_totals(db: &Connection, config: &Config, now: DateTime<Utc>) -> Result<()> {
    let today = Local::now().date_naive();

    db.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<()> {
        rebuild_one_day(db, config, today - Duration::days(1), now)?;
        rebuild_one_day(db, config, today, now)?;
        Ok(())
    })();
    match &result {
        Ok(_) => db.execute_batch("COMMIT;")?,
        Err(_) => {
            let _ = db.execute_batch("ROLLBACK;");
        }
    }
    result
}

fn rebuild_one_day(
    db: &Connection,
    config: &Config,
    day: NaiveDate,
    now: DateTime<Utc>,
) -> Result<()> {
    let day_string = day.to_string();
    let start = local_day_start_utc(day)?;
    let end = start + Duration::days(1);
    let effective_now = now.min(end);
    rebuild_daily_totals_inner(db, config, &day_string, start, end, effective_now)
}

fn rebuild_daily_totals_inner(
    db: &Connection,
    config: &Config,
    day_string: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    effective_now: DateTime<Utc>,
) -> Result<()> {
    db.execute(
        "DELETE FROM daily_app_totals WHERE day = ?1",
        params![day_string],
    )?;
    db.execute(
        "DELETE FROM daily_domain_totals WHERE day = ?1",
        params![day_string],
    )?;

    let mut app_totals: HashMap<String, i64> = HashMap::new();
    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, app_id
         FROM app_intervals
         WHERE started_at < ?1 AND COALESCE(ended_at, ?2) > ?3",
    )?;
    let rows = stmt.query_map(
        params![
            end.to_rfc3339(),
            effective_now.to_rfc3339(),
            start.to_rfc3339()
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (started_at, ended_at, app_id) = row?;
        let raw_interval_start = parse_rfc3339_utc(&started_at)?;
        let interval_start = raw_interval_start.max(start);
        let interval_end = capped_interval_end(
            raw_interval_start,
            ended_at
                .map(|value| parse_rfc3339_utc(&value))
                .transpose()?,
            effective_now,
            config.idle_after_secs,
        )
        .min(effective_now);
        if interval_end > interval_start {
            *app_totals.entry(app_id).or_default() += (interval_end - interval_start).num_seconds();
        }
    }

    for (app_id, seconds) in app_totals {
        let category = config.category_for_app(&app_id);
        db.execute(
            "INSERT INTO daily_app_totals(day, app_id, seconds, watch_category)
             VALUES (?1, ?2, ?3, ?4)",
            params![day_string, app_id, seconds, category],
        )?;
    }

    let mut domain_totals: HashMap<String, i64> = HashMap::new();
    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, domain
         FROM domain_intervals
         WHERE started_at < ?1 AND ended_at > ?2",
    )?;
    let rows = stmt.query_map(params![end.to_rfc3339(), start.to_rfc3339()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (started_at, ended_at, domain) = row?;
        let interval_start = parse_rfc3339_utc(&started_at)?.max(start);
        let interval_end = parse_rfc3339_utc(&ended_at)?.min(end);
        if interval_end > interval_start {
            *domain_totals.entry(domain).or_default() +=
                (interval_end - interval_start).num_seconds();
        }
    }

    for (domain, seconds) in domain_totals {
        let category = config.category_for_domain(&domain);
        db.execute(
            "INSERT INTO daily_domain_totals(day, domain, seconds, watch_category)
             VALUES (?1, ?2, ?3, ?4)",
            params![day_string, domain, seconds, category],
        )?;
    }

    Ok(())
}

fn load_app_status(db: &Connection, day: &str) -> Result<Vec<AppStatus>> {
    let mut stmt = db.prepare(
        "SELECT app_id, seconds, watch_category
         FROM daily_app_totals
         WHERE day = ?1 AND seconds > 0
         ORDER BY seconds DESC, app_id ASC",
    )?;
    let rows = stmt.query_map(params![day], |row| {
        let category: Option<String> = row.get(2)?;
        Ok(AppStatus {
            id: row.get(0)?,
            seconds: row.get(1)?,
            watched: category.is_some(),
            category,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn load_domain_status(
    db: &Connection,
    day: &str,
    display: &DisplayConfig,
) -> Result<Vec<DomainStatus>> {
    let min_seconds = display.domains_min_seconds.max(0);
    let mut stmt = db.prepare(
        "SELECT domain, seconds, watch_category
         FROM daily_domain_totals
         WHERE day = ?1 AND seconds >= ?2
         ORDER BY seconds DESC, domain ASC",
    )?;
    let rows = stmt.query_map(params![day, min_seconds.max(1)], |row| {
        let category: Option<String> = row.get(2)?;
        Ok(DomainStatus {
            domain: row.get(0)?,
            seconds: row.get(1)?,
            watched: category.is_some(),
            category,
        })
    })?;
    let mut domains = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let watched_idx: Vec<usize> = domains
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.watched.then_some(i))
        .collect();
    if display.domains_show_top > 0 && domains.len() > display.domains_show_top {
        let mut keep: HashSet<usize> = (0..display.domains_show_top).collect();
        keep.extend(watched_idx);
        let mut filtered = Vec::with_capacity(keep.len());
        for (i, d) in domains.drain(..).enumerate() {
            if keep.contains(&i) {
                filtered.push(d);
            }
        }
        domains = filtered;
    }
    Ok(domains)
}

fn import_domains_for_today(state: &AppState) -> Result<()> {
    let config = state.config.lock().unwrap().clone();
    let day = Local::now().date_naive();
    let imports = collect_browser_imports_for_day(&config, day)?;
    write_collected_domain_imports(state, &config, day, &imports).map(|_| ())
}

fn write_collected_domain_imports(
    state: &AppState,
    collected_config: &Config,
    day: NaiveDate,
    imports: &[BrowserImport],
) -> Result<bool> {
    let db = state.db.lock().unwrap();
    let current_state_path = state.db_state_path.lock().unwrap().clone();
    if current_state_path != collected_config.state_path {
        return Ok(false);
    }
    write_domain_imports_for_day(&db, collected_config, day, imports)?;
    Ok(true)
}

#[derive(Debug)]
struct BrowserImport {
    name: String,
    browser: BrowserConfig,
    successful_profiles: Vec<String>,
    visits: Vec<BrowserVisit>,
}

fn collect_browser_imports_for_day(config: &Config, day: NaiveDate) -> Result<Vec<BrowserImport>> {
    let start = local_day_start_utc(day)?;
    let end = start + Duration::days(1);
    let mut imports = Vec::new();

    for (name, browser) in &config.browsers {
        if browser.kind != "chromium" {
            continue;
        }
        let history_paths = match discover_history_paths(browser) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("attn could not discover {name} browser history: {error:#}");
                continue;
            }
        };
        let mut successful_profiles = Vec::new();
        let mut visits = Vec::new();
        for path in history_paths {
            match read_chromium_visits(&path, name, start, end) {
                Ok(profile_visits) => {
                    successful_profiles.push(source_profile_from_history_path(&path));
                    visits.extend(profile_visits);
                }
                Err(error) => eprintln!(
                    "attn could not read browser history {}: {error:#}",
                    path.display()
                ),
            }
        }
        if !successful_profiles.is_empty() {
            successful_profiles.sort();
            successful_profiles.dedup();
            imports.push(BrowserImport {
                name: name.clone(),
                browser: browser.clone(),
                successful_profiles,
                visits,
            });
        }
    }

    Ok(imports)
}

fn write_domain_imports_for_day(
    db: &Connection,
    config: &Config,
    day: NaiveDate,
    imports: &[BrowserImport],
) -> Result<()> {
    let start = local_day_start_utc(day)?;
    let end = start + Duration::days(1);

    for import in imports {
        delete_imported_domain_intervals(db, import, start, end)?;
        for visit in &import.visits {
            insert_attributed_domain_intervals(
                db,
                visit,
                &import.name,
                &import.browser,
                start,
                end,
                config.idle_after_secs,
            )?;
        }
    }

    Ok(())
}

fn delete_imported_domain_intervals(
    db: &Connection,
    import: &BrowserImport,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    for source_profile in &import.successful_profiles {
        db.execute(
            "DELETE FROM domain_intervals
             WHERE browser_name = ?1
               AND source_profile = ?2
               AND started_at < ?3
               AND ended_at > ?4",
            params![
                import.name,
                source_profile,
                end.to_rfc3339(),
                start.to_rfc3339()
            ],
        )?;
        for app_id in &import.browser.app_ids {
            db.execute(
                "DELETE FROM domain_intervals
                 WHERE browser_name = ''
                   AND browser_app_id = ?1
                   AND source_profile = ?2
                   AND started_at < ?3
                   AND ended_at > ?4",
                params![app_id, source_profile, end.to_rfc3339(), start.to_rfc3339()],
            )?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct BrowserVisit {
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    url: String,
    domain: String,
    source_profile: String,
}

fn discover_history_paths(browser: &BrowserConfig) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for pattern in &browser.history_paths {
        let expanded = expand_path(pattern);
        let pattern = expanded.to_string_lossy().to_string();
        for entry in glob(&pattern).with_context(|| format!("invalid glob {pattern}"))? {
            match entry {
                Ok(path) if path.is_file() => paths.push(path),
                Ok(_) => {}
                Err(error) => eprintln!("attn history glob error: {error}"),
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn read_chromium_visits(
    history_path: &Path,
    browser_name: &str,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<BrowserVisit>> {
    let snapshot = snapshot_history(history_path, browser_name)?;
    let source_profile = source_profile_from_history_path(history_path);
    let result =
        read_chromium_visits_from_snapshot(&snapshot, &source_profile, window_start, window_end);
    remove_snapshot_files(&snapshot);
    result
}

fn cleanup_stale_snapshots() {
    let temp_dir = env::temp_dir();
    let entries = match fs::read_dir(&temp_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let stale_threshold = std::time::SystemTime::now() - StdDuration::from_secs(3600);
    let my_uid = unsafe { libc_getuid() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("attn-") {
            continue;
        }
        let path = entry.path();
        if let Ok(metadata) = entry.metadata() {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != my_uid {
                continue;
            }
            if let Ok(mtime) = metadata.modified() {
                if mtime > stale_threshold {
                    continue;
                }
            }
            let _ = fs::remove_file(&path);
        }
    }
}

extern "C" {
    fn getuid() -> u32;
}

unsafe fn libc_getuid() -> u32 {
    getuid()
}

fn snapshot_history(history_path: &Path, browser_name: &str) -> Result<PathBuf> {
    let mut path = env::temp_dir();
    let unique = format!(
        "attn-{browser_name}-{}-{}.sqlite",
        std::process::id(),
        Utc::now().timestamp_micros()
    );
    path.push(unique);
    fs::copy(history_path, &path).with_context(|| {
        format!(
            "failed to snapshot browser history {}",
            history_path.display()
        )
    })?;
    harden_file_permissions(&path)?;
    copy_sqlite_sidecar(history_path, &path, "-wal")?;
    copy_sqlite_sidecar(history_path, &path, "-shm")?;
    Ok(path)
}

fn copy_sqlite_sidecar(source_db: &Path, snapshot_db: &Path, suffix: &str) -> Result<()> {
    let source = sqlite_sidecar_path(source_db, suffix);
    if source.exists() {
        let destination = sqlite_sidecar_path(snapshot_db, suffix);
        fs::copy(&source, &destination)
            .with_context(|| format!("failed to snapshot SQLite sidecar {}", source.display()))?;
        harden_file_permissions(&destination)?;
    }
    Ok(())
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

fn remove_snapshot_files(snapshot: &Path) {
    let _ = fs::remove_file(snapshot);
    let _ = fs::remove_file(sqlite_sidecar_path(snapshot, "-wal"));
    let _ = fs::remove_file(sqlite_sidecar_path(snapshot, "-shm"));
}

fn read_chromium_visits_from_snapshot(
    snapshot: &Path,
    source_profile: &str,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<BrowserVisit>> {
    let db = Connection::open(snapshot)
        .with_context(|| format!("failed to open history snapshot {}", snapshot.display()))?;
    let chrome_start = utc_to_chrome_micros(window_start - Duration::hours(12));
    let chrome_end = utc_to_chrome_micros(window_end + Duration::hours(12));
    let mut stmt = db.prepare(
        "SELECT visits.visit_time, COALESCE(visits.visit_duration, 0), urls.url
         FROM visits
         JOIN urls ON urls.id = visits.url
         WHERE visits.visit_time >= ?1 AND visits.visit_time <= ?2
         ORDER BY visits.visit_time ASC",
    )?;
    let rows = stmt.query_map(params![chrome_start, chrome_end], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut visits = Vec::new();
    for row in rows {
        let (visit_time, visit_duration, raw_url) = row?;
        let Some(domain) = domain_from_url(&raw_url) else {
            continue;
        };
        let started_at = chrome_micros_to_utc(visit_time)?;
        let duration_secs = (visit_duration / 1_000_000).clamp(1, 900);
        let ended_at = started_at + Duration::seconds(duration_secs);
        if ended_at <= window_start || started_at >= window_end {
            continue;
        }
        visits.push(BrowserVisit {
            started_at,
            ended_at,
            url: raw_url,
            domain,
            source_profile: source_profile.to_string(),
        });
    }
    Ok(cap_visits_at_next_start(visits))
}

fn cap_visits_at_next_start(mut visits: Vec<BrowserVisit>) -> Vec<BrowserVisit> {
    visits.sort_by_key(|visit| visit.started_at);
    let starts = visits
        .iter()
        .map(|visit| visit.started_at)
        .collect::<Vec<_>>();

    for (index, visit) in visits.iter_mut().enumerate() {
        if let Some(next_start) = starts.get(index + 1) {
            if *next_start > visit.started_at && *next_start < visit.ended_at {
                visit.ended_at = *next_start;
            }
        }
    }

    visits
        .into_iter()
        .filter(|visit| visit.ended_at > visit.started_at)
        .collect()
}

fn source_profile_from_history_path(history_path: &Path) -> String {
    history_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn insert_attributed_domain_intervals(
    db: &Connection,
    visit: &BrowserVisit,
    browser_name: &str,
    browser: &BrowserConfig,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    idle_after_secs: i64,
) -> Result<()> {
    let app_ids = browser
        .app_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if app_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", app_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT started_at, ended_at, app_id
         FROM app_intervals
         WHERE app_id IN ({placeholders})
           AND started_at < ?
           AND COALESCE(ended_at, ?) > ?"
    );

    let mut values: Vec<&dyn rusqlite::ToSql> = Vec::new();
    for app_id in &app_ids {
        values.push(app_id);
    }
    let visit_end = visit.ended_at.to_rfc3339();
    let visit_start = visit.started_at.to_rfc3339();
    let window_end_text = window_end.to_rfc3339();
    values.push(&visit_end);
    values.push(&window_end_text);
    values.push(&visit_start);

    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(values.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    for row in rows {
        let (focus_start, focus_end, app_id) = row?;
        let raw_focus_start = parse_rfc3339_utc(&focus_start)?;
        let focus_start = raw_focus_start.max(window_start);
        let focus_end = capped_interval_end(
            raw_focus_start,
            focus_end
                .map(|value| parse_rfc3339_utc(&value))
                .transpose()?,
            window_end,
            idle_after_secs,
        )
        .min(window_end);
        let started_at = visit.started_at.max(focus_start);
        let ended_at = visit.ended_at.min(focus_end);
        if ended_at <= started_at {
            continue;
        }
        insert_uncovered_domain_segments(db, visit, browser_name, &app_id, started_at, ended_at)?;
    }

    Ok(())
}

fn insert_uncovered_domain_segments(
    db: &Connection,
    visit: &BrowserVisit,
    browser_name: &str,
    browser_app_id: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
) -> Result<()> {
    for (segment_start, segment_end) in
        uncovered_domain_segments(db, browser_app_id, started_at, ended_at)?
    {
        db.execute(
            "INSERT INTO domain_intervals(
                started_at, ended_at, browser_name, browser_app_id, domain, url, source_profile
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                segment_start.to_rfc3339(),
                segment_end.to_rfc3339(),
                browser_name,
                browser_app_id,
                visit.domain,
                visit.url,
                visit.source_profile,
            ],
        )?;
    }

    Ok(())
}

fn uncovered_domain_segments(
    db: &Connection,
    browser_app_id: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let mut segments = vec![(started_at, ended_at)];
    let mut stmt = db.prepare(
        "SELECT started_at, ended_at
         FROM domain_intervals
         WHERE browser_app_id = ?1
           AND started_at < ?2
           AND ended_at > ?3
         ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map(
        params![
            browser_app_id,
            ended_at.to_rfc3339(),
            started_at.to_rfc3339()
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;

    for row in rows {
        let (occupied_start, occupied_end) = row?;
        let occupied_start = parse_rfc3339_utc(&occupied_start)?;
        let occupied_end = parse_rfc3339_utc(&occupied_end)?;
        let mut next_segments = Vec::new();

        for (segment_start, segment_end) in segments {
            if occupied_end <= segment_start || occupied_start >= segment_end {
                next_segments.push((segment_start, segment_end));
                continue;
            }

            let before_end = occupied_start.min(segment_end);
            if segment_start < before_end {
                next_segments.push((segment_start, before_end));
            }

            let after_start = occupied_end.max(segment_start);
            if after_start < segment_end {
                next_segments.push((after_start, segment_end));
            }
        }

        segments = next_segments;
        if segments.is_empty() {
            break;
        }
    }

    Ok(segments)
}

fn run_doctor() -> Result<()> {
    let mut ok = true;
    let mut warnings: Vec<String> = Vec::new();
    let config = match Config::load_default() {
        Ok(config) => {
            println!("config: ok");
            config
        }
        Err(error) => {
            println!("config: error: {error:#}");
            ok = false;
            Config::default()
        }
    };

    if config.breaks.interval_secs < 60 {
        warnings.push(format!(
            "breaks.interval_secs is {} (very short); typical values are 1800-7200",
            config.breaks.interval_secs
        ));
    }
    if config.breaks.min_break_secs < 30 {
        warnings.push(format!(
            "breaks.min_break_secs is {} (very short); typical values are 120-600",
            config.breaks.min_break_secs
        ));
    }

    match ensure_parent_dir(&config.state_path)
        .and_then(|_| Connection::open(&config.state_path).map_err(Into::into))
        .and_then(|db| migrate(&db))
        .and_then(|_| harden_sqlite_permissions(&config.state_path))
    {
        Ok(()) => println!("state_db: ok ({})", config.state_path.display()),
        Err(error) => {
            println!("state_db: error: {error:#}");
            ok = false;
        }
    }

    {
        let kind = &config.focus_source.kind;
        let resolved = if kind == "auto" { focus::auto_detect() } else { kind.as_str() };
        match focus::build(resolved) {
            Ok(source) => {
                let probe = source.poll_current();
                match probe {
                    Ok(_) => println!("focus_source: {} (kind = {kind})", source.name()),
                    Err(error) => {
                        println!("focus_source: {} (kind = {kind}) — probe failed: {error:#}", source.name());
                        warnings.push(format!("focus source probe failed: {error:#}"));
                    }
                }
            }
            Err(error) => {
                println!("focus_source: error building source (kind = {kind}): {error:#}");
                ok = false;
            }
        }
    }

    match probe_wayland_idle() {
        Ok(true) => println!("wayland idle-notify: ok"),
        Ok(false) => {
            println!("wayland idle-notify: unavailable (compositor missing ext_idle_notifier_v1)");
            warnings.push(
                "auto-pause on idle disabled; manual break-start/break-end still work".into(),
            );
        }
        Err(error) => {
            println!("wayland idle-notify: error: {error:#}");
            warnings.push(format!("wayland idle probe failed: {error:#}"));
        }
    }

    match probe_dbus_login1() {
        Ok(true) => println!("dbus login1: ok"),
        Ok(false) => {
            println!("dbus login1: unreachable");
            warnings.push("suspend/wake detection limited to heartbeat fallback".into());
        }
        Err(error) => {
            println!("dbus login1: error: {error:#}");
        }
    }

    for (name, browser) in &config.browsers {
        match discover_history_paths(browser) {
            Ok(paths) if paths.is_empty() => {
                println!("browser.{name}: no history DBs found");
            }
            Ok(paths) => {
                println!("browser.{name}: {} history DB(s)", paths.len());
                for path in paths {
                    println!("  {}", path.display());
                }
            }
            Err(error) => {
                println!("browser.{name}: error: {error:#}");
                ok = false;
            }
        }
    }

    match check_socket_path(&config.socket_path) {
        Ok(SocketPathStatus::DaemonReachable) => {
            println!(
                "socket: daemon reachable ({})",
                config.socket_path.display()
            );
        }
        Ok(SocketPathStatus::Available) => {
            println!("socket: available ({})", config.socket_path.display());
        }
        Ok(SocketPathStatus::StaleOrBlocked(error)) => {
            println!(
                "socket: stale or blocked ({}) ({error})",
                config.socket_path.display()
            );
            ok = false;
        }
        Err(error) => {
            println!("socket: error: {error:#}");
            ok = false;
        }
    }

    for w in &warnings {
        println!("warning: {w}");
    }

    if ok {
        println!("\nverdict: ok");
        Ok(())
    } else {
        println!("\nverdict: errors found");
        bail!("doctor found errors")
    }
}

fn probe_wayland_idle() -> Result<bool> {
    use wayland_client::Connection as WConnection;
    let conn = match WConnection::connect_to_env() {
        Ok(c) => c,
        Err(error) => bail!("wayland connect failed: {error:#}"),
    };
    let mut event_queue = conn.new_event_queue::<DoctorIdleProbe>();
    let qh = event_queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());
    let mut probe = DoctorIdleProbe { found: false };
    event_queue.roundtrip(&mut probe).ok();
    event_queue.roundtrip(&mut probe).ok();
    Ok(probe.found)
}

struct DoctorIdleProbe {
    found: bool,
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()>
    for DoctorIdleProbe
{
    fn event(
        probe: &mut Self,
        _registry: &wayland_client::protocol::wl_registry::WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _data: &(),
        _conn: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::Global { interface, .. } = event {
            if interface == "ext_idle_notifier_v1" {
                probe.found = true;
            }
        }
    }
}

fn probe_dbus_login1() -> Result<bool> {
    use zbus::blocking::Connection as ZConnection;
    let conn = match ZConnection::system() {
        Ok(c) => c,
        Err(error) => bail!("system bus connect failed: {error:#}"),
    };
    let proxy = match zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    ) {
        Ok(p) => p,
        Err(error) => bail!("login1 proxy failed: {error:#}"),
    };
    Ok(proxy.introspect().is_ok())
}

enum SocketPathStatus {
    DaemonReachable,
    Available,
    StaleOrBlocked(String),
}

fn check_socket_path(socket_path: &Path) -> Result<SocketPathStatus> {
    ensure_parent_dir(socket_path)?;
    match UnixStream::connect(socket_path) {
        Ok(_) => Ok(SocketPathStatus::DaemonReachable),
        Err(_) if !socket_path.exists() => Ok(SocketPathStatus::Available),
        Err(error) => Ok(SocketPathStatus::StaleOrBlocked(error.to_string())),
    }
}

fn local_day_start_utc(day: NaiveDate) -> Result<DateTime<Utc>> {
    let local = Local
        .from_local_datetime(
            &day.and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow!("invalid local day"))?,
        )
        .single()
        .ok_or_else(|| anyhow!("could not resolve local midnight"))?;
    Ok(local.with_timezone(&Utc))
}

fn parse_rfc3339_utc(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn capped_interval_end(
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    fallback_end: DateTime<Utc>,
    idle_after_secs: i64,
) -> DateTime<Utc> {
    let raw_end = ended_at.unwrap_or(fallback_end);
    raw_end
        .min(started_at + Duration::seconds(idle_after_secs.max(1)))
        .max(started_at)
}

fn chrome_micros_to_utc(value: i64) -> Result<DateTime<Utc>> {
    let unix_micros = value - CHROME_EPOCH_OFFSET_MICROS;
    Utc.timestamp_micros(unix_micros)
        .single()
        .ok_or_else(|| anyhow!("invalid Chromium timestamp {value}"))
}

fn utc_to_chrome_micros(value: DateTime<Utc>) -> i64 {
    value.timestamp_micros() + CHROME_EPOCH_OFFSET_MICROS
}

fn domain_from_url(raw_url: &str) -> Option<String> {
    let url = Url::parse(raw_url).ok()?;
    let host = url.host_str()?;
    Some(normalize_domain(host))
}

fn normalize_domain(domain: &str) -> String {
    domain
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("www.")
        .to_ascii_lowercase()
}

fn normalize_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

fn domain_matches(domain: &str, candidate: &str) -> bool {
    let domain = normalize_domain(domain);
    let candidate = normalize_domain(candidate);
    domain == candidate || domain.ends_with(&format!(".{candidate}"))
}

fn expand_path(value: &str) -> PathBuf {
    let mut expanded = value.to_string();
    if let Some(home) = env::var_os("HOME").and_then(|home| home.into_string().ok()) {
        if expanded == "~" {
            expanded = home;
        } else if let Some(rest) = expanded.strip_prefix("~/") {
            expanded = format!("{home}/{rest}");
        }
    }
    for (key, value) in env::vars() {
        expanded = expanded.replace(&format!("${key}"), &value);
    }
    PathBuf::from(expanded)
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

fn harden_sqlite_permissions(path: &Path) -> Result<()> {
    harden_file_permissions(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path(path, suffix);
        if sidecar.exists() {
            harden_file_permissions(&sidecar)?;
        }
    }
    Ok(())
}

fn harden_file_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set private permissions on {}", path.display()))
}

fn bool_to_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_matching_is_suffix_safe() {
        assert!(domain_matches("youtube.com", "youtube.com"));
        assert!(domain_matches("www.youtube.com", "youtube.com"));
        assert!(domain_matches("m.youtube.com", "youtube.com"));
        assert!(!domain_matches("notyoutube.com", "youtube.com"));
    }

    #[test]
    fn chrome_timestamp_round_trips() {
        let now = Utc::now();
        let rounded = now
            .timestamp_micros()
            .checked_div(1)
            .and_then(|micros| Utc.timestamp_micros(micros).single())
            .unwrap();
        let chrome = utc_to_chrome_micros(rounded);
        assert_eq!(chrome_micros_to_utc(chrome).unwrap(), rounded);
    }

    #[test]
    fn niri_focus_event_detection_uses_top_level_event_names() {
        use crate::focus::niri::is_focus_event;
        assert!(is_focus_event(r#"{"WindowFocusChanged":{"id":42}}"#));
        assert!(is_focus_event(
            r#"{"WorkspaceActiveWindowChanged":{"workspace_id":1,"active_window_id":42}}"#
        ));
        assert!(is_focus_event(
            r#"{"WorkspaceActivated":{"id":1,"focused":true}}"#
        ));
        assert!(!is_focus_event(
            r#"{"WindowOpenedOrChanged":{"window":{"id":9,"is_focused":true,"title":"spinner"}}}"#
        ));
        assert!(!is_focus_event(
            r#"{"WindowsChanged":{"windows":[{"id":9,"is_focused":true}]}}"#
        ));
        assert!(!is_focus_event("not-json"));
    }

    #[test]
    fn open_interval_matching_uses_window_id_when_available() {
        let open = OpenAppInterval {
            window_id: Some(1),
            app_id: "com.mitchellh.ghostty".to_string(),
        };
        assert!(open.matches(&FocusedWindow {
            window_id: Some(1),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "one".to_string(),
            pid: None,
        }));
        assert!(!open.matches(&FocusedWindow {
            window_id: Some(2),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "two".to_string(),
            pid: None,
        }));
        assert!(!open.matches(&FocusedWindow {
            window_id: Some(1),
            app_id: "claude".to_string(),
            title: "ghostty running claude".to_string(),
            pid: None,
        }));
    }

    #[test]
    fn pick_terminal_subprocess_prefers_most_recent_start_time() {
        let config = Config::default();
        let candidates = vec![
            DescendantProc { pid: 11, depth: 1, name: "bash".to_string(), start_time: 100 },
            DescendantProc { pid: 12, depth: 2, name: "claude".to_string(), start_time: 500 },
            DescendantProc { pid: 13, depth: 3, name: "codex".to_string(), start_time: 300 },
            DescendantProc { pid: 14, depth: 4, name: "node".to_string(), start_time: 1000 },
        ];
        assert_eq!(pick_terminal_subprocess(&candidates, &config), Some("claude".to_string()));
    }

    #[test]
    fn pick_terminal_subprocess_returns_none_when_no_match() {
        let config = Config::default();
        let candidates = vec![
            DescendantProc { pid: 11, depth: 1, name: "bash".to_string(), start_time: 100 },
            DescendantProc { pid: 12, depth: 2, name: "fish".to_string(), start_time: 200 },
        ];
        assert_eq!(pick_terminal_subprocess(&candidates, &config), None);
    }

    #[test]
    fn title_match_finds_program_word() {
        let config = Config::default();
        assert_eq!(
            match_terminal_program_in_title("~/code/foo - claude", &config),
            Some("claude".to_string())
        );
        assert_eq!(
            match_terminal_program_in_title("codex repl", &config),
            Some("codex".to_string())
        );
        assert_eq!(
            match_terminal_program_in_title("clauderyx", &config),
            None
        );
        assert_eq!(
            match_terminal_program_in_title("", &config),
            None
        );
    }

    #[test]
    fn category_for_app_falls_back_to_terminal_apps() {
        let config = Config::default();
        assert_eq!(config.category_for_app("claude"), Some("ai".to_string()));
        assert_eq!(config.category_for_app("nvim"), Some("editor".to_string()));
    }

    #[test]
    fn is_terminal_app_matches_watch_terminal_list() {
        let config = Config::default();
        assert!(config.is_terminal_app("com.mitchellh.ghostty"));
        assert!(config.is_terminal_app("wezterm"));
        assert!(!config.is_terminal_app("brave"));
    }

    #[test]
    fn resolve_focused_window_rewrites_terminal_app_id() {
        let config = Config::default();
        let titled = FocusedWindow {
            window_id: Some(7),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "claude".to_string(),
            pid: None,
        };
        let resolved = resolve_focused_window(titled, &config);
        assert_eq!(resolved.app_id, "claude");

        let untitled = FocusedWindow {
            window_id: Some(7),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "tmux attach -t empyreal".to_string(),
            pid: None,
        };
        let resolved = resolve_focused_window(untitled, &config);
        assert_eq!(resolved.app_id, "com.mitchellh.ghostty");

        let non_terminal = FocusedWindow {
            window_id: Some(7),
            app_id: "brave".to_string(),
            title: "x".to_string(),
            pid: Some(1234),
        };
        let resolved = resolve_focused_window(non_terminal, &config);
        assert_eq!(resolved.app_id, "brave");
    }

    #[test]
    fn migration_creates_window_id_column() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        assert!(table_column_exists(&db, "app_intervals", "window_id").unwrap());
        assert!(table_column_exists(&db, "domain_intervals", "browser_name").unwrap());
    }

    #[test]
    fn partial_config_keeps_default_watch_lists_and_browsers() {
        let path = unique_temp_db_path("attn-config");
        fs::write(
            &path,
            r#"
            poll_interval_secs = 15
            "#,
        )
        .unwrap();
        let config = Config::load_from_path(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(
            config.category_for_app("com.mitchellh.ghostty").as_deref(),
            Some("terminal")
        );
        assert_eq!(
            config.category_for_domain("www.youtube.com").as_deref(),
            Some("video")
        );
        assert!(config.browsers.contains_key("brave"));
        assert!(config.browsers.contains_key("helium"));
    }

    #[test]
    fn config_default_uses_generated_default_toml() {
        let config = Config::default();
        assert_eq!(config.category_for_app("positron"), Some("coding".to_string()));
        assert_eq!(config.category_for_domain("vercel.com"), Some("coding".to_string()));
    }

    #[test]
    fn user_watch_lists_add_to_bundled_defaults() {
        let path = unique_temp_db_path("attn-merged-config");
        fs::write(
            &path,
            r#"
            [apps.watch]
            coding = ["my-editor"]

            [domains.watch]
            learning = ["example.edu"]
            "#,
        )
        .unwrap();
        let config = Config::load_from_path(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(config.category_for_app("my-editor"), Some("coding".to_string()));
        assert_eq!(config.category_for_app("positron"), Some("coding".to_string()));
        assert_eq!(
            config.category_for_domain("example.edu"),
            Some("learning".to_string())
        );
        assert_eq!(config.category_for_domain("vercel.com"), Some("coding".to_string()));
    }

    #[test]
    fn invalid_config_rejects_zero_poll_interval() {
        let path = unique_temp_db_path("attn-invalid-config");
        fs::write(
            &path,
            r#"
            poll_interval_secs = 0
            "#,
        )
        .unwrap();
        let error = Config::load_from_path(&path).unwrap_err().to_string();
        let _ = fs::remove_file(&path);
        assert!(error.contains("poll_interval_secs"));
    }

    #[test]
    fn attribution_clips_domain_visits_to_browser_focus() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(10);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'Example')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let visit = BrowserVisit {
            started_at: focus_start - Duration::minutes(5),
            ended_at: focus_start + Duration::minutes(12),
            url: "https://www.youtube.com/watch?v=test".to_string(),
            domain: "youtube.com".to_string(),
            source_profile: "Default".to_string(),
        };
        insert_attributed_domain_intervals(
            &db,
            &visit,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            1200,
        )
        .unwrap();

        let seconds: i64 = db
            .query_row(
                "SELECT CAST((julianday(ended_at) - julianday(started_at)) * 86400 AS INTEGER)
                 FROM domain_intervals",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seconds, 600);
    }

    #[test]
    fn attribution_caps_browser_focus_at_idle_threshold() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(30);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'Example')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let visit = BrowserVisit {
            started_at: focus_start,
            ended_at: focus_start + Duration::minutes(30),
            url: "https://www.youtube.com/watch?v=test".to_string(),
            domain: "youtube.com".to_string(),
            source_profile: "Default".to_string(),
        };
        insert_attributed_domain_intervals(
            &db,
            &visit,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            300,
        )
        .unwrap();

        let seconds: i64 = db
            .query_row(
                "SELECT CAST((julianday(ended_at) - julianday(started_at)) * 86400 AS INTEGER)
                 FROM domain_intervals",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seconds, 300);
    }

    #[test]
    fn browser_visits_are_capped_at_next_visit_start() {
        let started_at = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let visits = cap_visits_at_next_start(vec![
            BrowserVisit {
                started_at,
                ended_at: started_at + Duration::minutes(10),
                url: "https://youtube.com/".to_string(),
                domain: "youtube.com".to_string(),
                source_profile: "Default".to_string(),
            },
            BrowserVisit {
                started_at: started_at + Duration::minutes(5),
                ended_at: started_at + Duration::minutes(15),
                url: "https://reddit.com/".to_string(),
                domain: "reddit.com".to_string(),
                source_profile: "Default".to_string(),
            },
        ]);

        assert_eq!(
            (visits[0].ended_at - visits[0].started_at).num_seconds(),
            300
        );
        assert_eq!(
            (visits[1].ended_at - visits[1].started_at).num_seconds(),
            600
        );
    }

    #[test]
    fn overlapping_domain_intervals_do_not_exceed_browser_focus_time() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(10);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'Example')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let first = BrowserVisit {
            started_at: focus_start,
            ended_at: focus_start + Duration::minutes(7),
            url: "https://youtube.com/".to_string(),
            domain: "youtube.com".to_string(),
            source_profile: "Default".to_string(),
        };
        let second = BrowserVisit {
            started_at: focus_start + Duration::minutes(5),
            ended_at: focus_end,
            url: "https://reddit.com/".to_string(),
            domain: "reddit.com".to_string(),
            source_profile: "Default".to_string(),
        };

        insert_attributed_domain_intervals(
            &db,
            &first,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            1200,
        )
        .unwrap();
        insert_attributed_domain_intervals(
            &db,
            &second,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            1200,
        )
        .unwrap();

        let seconds: i64 = db
            .query_row(
                "SELECT CAST(SUM((julianday(ended_at) - julianday(started_at)) * 86400) AS INTEGER)
                 FROM domain_intervals",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seconds, 600);
    }

    #[test]
    fn write_domain_imports_rebuilds_from_collected_visits() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config::default();
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(10);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'Example')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO domain_intervals(started_at, ended_at, browser_app_id, domain, url, source_profile)
             VALUES (?1, ?2, 'brave-browser', 'old.example', 'https://old.example/', 'Default')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: vec![BrowserVisit {
                started_at: focus_start,
                ended_at: focus_start + Duration::minutes(3),
                url: "https://youtube.com/".to_string(),
                domain: "youtube.com".to_string(),
                source_profile: "Default".to_string(),
            }],
        }];

        write_domain_imports_for_day(
            &db,
            &config,
            focus_start.with_timezone(&Local).date_naive(),
            &imports,
        )
        .unwrap();

        let domains = db
            .prepare("SELECT domain FROM domain_intervals ORDER BY domain")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(domains, vec!["youtube.com"]);
    }

    #[test]
    fn write_domain_imports_preserves_unscanned_profiles() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config::default();
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(10);
        for (domain, profile) in [
            ("old-default.example", "Default"),
            ("old-profile.example", "Profile 1"),
        ] {
            db.execute(
                "INSERT INTO domain_intervals(started_at, ended_at, browser_app_id, domain, url, source_profile)
                 VALUES (?1, ?2, 'brave-browser', ?3, ?4, ?5)",
                params![
                    focus_start.to_rfc3339(),
                    focus_end.to_rfc3339(),
                    domain,
                    format!("https://{domain}/"),
                    profile
                ],
            )
            .unwrap();
        }

        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: Vec::new(),
        }];

        write_domain_imports_for_day(
            &db,
            &config,
            focus_start.with_timezone(&Local).date_naive(),
            &imports,
        )
        .unwrap();

        let remaining = db
            .prepare("SELECT domain, source_profile FROM domain_intervals")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            remaining,
            vec![("old-profile.example".to_string(), "Profile 1".to_string())]
        );
    }

    #[test]
    fn write_domain_imports_rebuilds_browser_name_even_if_app_ids_change() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config::default();
        let browser = BrowserConfig {
            app_ids: vec!["new-brave".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(10);
        db.execute(
            "INSERT INTO domain_intervals(started_at, ended_at, browser_name, browser_app_id, domain, url, source_profile)
             VALUES (?1, ?2, 'brave', 'old-brave', 'old.example', 'https://old.example/', 'Default')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: Vec::new(),
        }];

        write_domain_imports_for_day(
            &db,
            &config,
            focus_start.with_timezone(&Local).date_naive(),
            &imports,
        )
        .unwrap();

        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM domain_intervals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn source_profile_comes_from_history_parent_directory() {
        let path = Path::new("/home/example/.config/BraveSoftware/Brave-Browser/Profile 1/History");
        assert_eq!(source_profile_from_history_path(path), "Profile 1");
    }

    #[test]
    fn sqlite_sidecar_paths_append_suffix_to_database_path() {
        let path = Path::new("/tmp/History");
        assert_eq!(
            sqlite_sidecar_path(path, "-wal"),
            PathBuf::from("/tmp/History-wal")
        );
        assert_eq!(
            sqlite_sidecar_path(path, "-shm"),
            PathBuf::from("/tmp/History-shm")
        );
    }

    #[test]
    fn harden_file_permissions_sets_user_only_mode() {
        let path = unique_temp_db_path("attn-permissions");
        fs::write(&path, b"private").unwrap();
        harden_file_permissions(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn open_state_db_creates_private_schema() {
        let path = unique_temp_db_path("attn-state-db");
        let db = open_state_db(&path).unwrap();
        assert!(table_column_exists(&db, "app_intervals", "window_id").unwrap());
        drop(db);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
        let _ = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn reopening_state_db_preserves_today_totals() {
        let path = unique_temp_db_path("attn-state-reopen");
        let now = Utc::now();
        let started_at = now - Duration::seconds(120);
        let ended_at = now - Duration::seconds(60);
        {
            let db = open_state_db(&path).unwrap();
            db.execute(
                "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
                 VALUES (?1, ?2, 'code', 'Editor')",
                params![started_at.to_rfc3339(), ended_at.to_rfc3339()],
            )
            .unwrap();
        }

        let db = open_state_db(&path).unwrap();
        let status = build_status(&db, &Config::default(), true).unwrap();
        drop(db);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(sqlite_sidecar_path(&path, "-wal"));
        let _ = fs::remove_file(sqlite_sidecar_path(&path, "-shm"));

        assert_eq!(status.watch_seconds, 60);
    }

    #[test]
    fn stale_open_intervals_are_capped_on_restart() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let now = Utc::now();
        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![(now - Duration::seconds(600)).to_rfc3339()],
        )
        .unwrap();

        close_stale_open_intervals(&db, now, 300).unwrap();

        let (started_at, ended_at, idle_adjusted): (String, String, i64) = db
            .query_row(
                "SELECT started_at,
                        ended_at,
                        idle_adjusted
                 FROM app_intervals",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let seconds = (parse_rfc3339_utc(&ended_at).unwrap()
            - parse_rfc3339_utc(&started_at).unwrap())
        .num_seconds();
        assert_eq!(seconds, 300);
        assert_eq!(idle_adjusted, 1);
    }

    #[test]
    fn reload_state_swap_caps_stale_intervals_in_new_db() {
        let old_path = unique_temp_db_path("attn-reload-old");
        let new_path = unique_temp_db_path("attn-reload-new");
        let now = Utc::now();
        let old_config = Config {
            state_path: old_path.clone(),
            idle_after_secs: 300,
            ..Default::default()
        };
        let new_config = Config {
            state_path: new_path.clone(),
            idle_after_secs: 120,
            ..Default::default()
        };

        let old_db = open_state_db(&old_path).unwrap();
        old_db
            .execute(
                "INSERT INTO app_intervals(started_at, app_id, window_title)
                 VALUES (?1, 'code', 'Old DB')",
                params![(now - Duration::seconds(600)).to_rfc3339()],
            )
            .unwrap();
        {
            let new_db = open_state_db(&new_path).unwrap();
            new_db
                .execute(
                    "INSERT INTO app_intervals(started_at, app_id, window_title)
                     VALUES (?1, 'code', 'New DB')",
                    params![(now - Duration::seconds(600)).to_rfc3339()],
                )
                .unwrap();
        }

        let state = AppState {
            config: Arc::new(Mutex::new(old_config.clone())),
            db: Arc::new(Mutex::new(old_db)),
            db_state_path: Arc::new(Mutex::new(old_config.state_path.clone())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
        };

        swap_state_db_for_reload(&state, &old_config, &new_config, now).unwrap();

        let db = state.db.lock().unwrap();
        let (started_at, ended_at, idle_adjusted): (String, String, i64) = db
            .query_row(
                "SELECT started_at, ended_at, idle_adjusted FROM app_intervals",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let seconds = (parse_rfc3339_utc(&ended_at).unwrap()
            - parse_rfc3339_utc(&started_at).unwrap())
        .num_seconds();
        drop(db);
        let _ = fs::remove_file(&old_path);
        let _ = fs::remove_file(sqlite_sidecar_path(&old_path, "-wal"));
        let _ = fs::remove_file(sqlite_sidecar_path(&old_path, "-shm"));
        let _ = fs::remove_file(&new_path);
        let _ = fs::remove_file(sqlite_sidecar_path(&new_path, "-wal"));
        let _ = fs::remove_file(sqlite_sidecar_path(&new_path, "-shm"));

        assert_eq!(seconds, 120);
        assert_eq!(idle_adjusted, 1);
    }

    #[test]
    fn stale_collected_imports_are_not_written_after_state_path_reload() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let old_config = Config {
            state_path: PathBuf::from("/tmp/attn-old.sqlite"),
            ..Default::default()
        };
        let new_config = Config {
            state_path: PathBuf::from("/tmp/attn-new.sqlite"),
            ..Default::default()
        };
        let state = AppState {
            config: Arc::new(Mutex::new(old_config.clone())),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(new_config.state_path.clone())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
        };

        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let started_at = Utc::now() - Duration::minutes(5);
        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: vec![BrowserVisit {
                started_at,
                ended_at: started_at + Duration::minutes(1),
                url: "https://youtube.com/".to_string(),
                domain: "youtube.com".to_string(),
                source_profile: "Default".to_string(),
            }],
        }];

        let wrote = write_collected_domain_imports(
            &state,
            &old_config,
            started_at.with_timezone(&Local).date_naive(),
            &imports,
        )
        .unwrap();

        let count: i64 = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM domain_intervals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!wrote);
        assert_eq!(count, 0);
    }

    #[test]
    fn closing_open_interval_does_not_end_before_start_after_clock_skew() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let started_at = Utc::now();
        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![started_at.to_rfc3339()],
        )
        .unwrap();

        close_open_interval(&db, started_at - Duration::seconds(60), 300).unwrap();

        let ended_at: String = db
            .query_row("SELECT ended_at FROM app_intervals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(parse_rfc3339_utc(&ended_at).unwrap(), started_at);
    }

    #[test]
    fn bound_socket_is_user_only() {
        let path = unique_temp_db_path("attn-socket");
        let listener = bind_socket(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        drop(listener);
        let _ = fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn socket_path_check_reports_available_when_absent() {
        let path = unique_temp_db_path("attn-socket-available");
        let status = check_socket_path(&path).unwrap();
        assert!(matches!(status, SocketPathStatus::Available));
    }

    #[test]
    fn socket_path_check_reports_reachable_daemon() {
        let path = unique_temp_db_path("attn-socket-reachable");
        let listener = bind_socket(&path).unwrap();
        let status = check_socket_path(&path).unwrap();
        drop(listener);
        let _ = fs::remove_file(&path);
        assert!(matches!(status, SocketPathStatus::DaemonReachable));
    }

    #[test]
    fn socket_client_ignores_empty_requests() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
        };
        let (client, server) = UnixStream::pair().unwrap();
        drop(client);

        handle_client(state, server).unwrap();
    }

    #[test]
    fn manual_break_start_converts_idle_pause() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let paused_at = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let idle_pause = PauseInfo {
            at: paused_at,
            reason: PauseReason::Idle,
        };
        persist_pause_state(&db, Some(idle_pause)).unwrap();

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(Some(idle_pause))),
            focus_source_kind: Arc::new("niri".to_string()),
        };

        let response = set_pause(&state, PauseReason::Manual).unwrap();
        assert!(response.ok);
        assert!(response.paused);
        assert_eq!(response.paused_reason.as_deref(), Some("manual"));
        assert_eq!(
            state.tracking_state.lock().unwrap().unwrap().reason,
            PauseReason::Manual
        );
        let persisted_reason = {
            let db = state.db.lock().unwrap();
            meta_get(&db, "paused_reason").unwrap()
        };
        assert_eq!(persisted_reason.as_deref(), Some("manual"));
    }

    #[test]
    fn active_session_counts_live_open_interval_past_idle_cap() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 11, 5, 0).unwrap();
        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![(now - Duration::minutes(65)).to_rfc3339()],
        )
        .unwrap();

        let seconds = compute_active_session_seconds(&db, now, 300, 300).unwrap();

        assert_eq!(seconds, 65 * 60);
    }

    #[test]
    fn chromium_reader_extracts_domains_and_caps_long_durations() {
        let path = unique_temp_db_path("attn-history-reader");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "
                CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT NOT NULL);
                CREATE TABLE visits(
                    id INTEGER PRIMARY KEY,
                    url INTEGER NOT NULL,
                    visit_time INTEGER NOT NULL,
                    visit_duration INTEGER NOT NULL
                );
                ",
            )
            .unwrap();
            let started_at = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
            db.execute(
                "INSERT INTO urls(id, url) VALUES (1, 'https://www.youtube.com/watch?v=test')",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO visits(id, url, visit_time, visit_duration)
                 VALUES (1, 1, ?1, ?2)",
                params![utc_to_chrome_micros(started_at), 3_600_i64 * 1_000_000],
            )
            .unwrap();
        }

        let started_at = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let visits = read_chromium_visits_from_snapshot(
            &path,
            "Default",
            started_at - Duration::minutes(1),
            started_at + Duration::minutes(20),
        )
        .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].domain, "youtube.com");
        assert_eq!(visits[0].source_profile, "Default");
        assert_eq!(
            (visits[0].ended_at - visits[0].started_at).num_seconds(),
            900
        );
    }

    #[test]
    fn status_counts_watched_apps_and_domains_without_browser_double_counting() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let mut config = Config::default();
        config
            .apps
            .watch
            .insert("browser".to_string(), vec!["brave-browser".to_string()]);
        let now = Utc::now();
        let app_start = now - Duration::seconds(180);
        let app_end = now - Duration::seconds(120);
        let browser_start = now - Duration::seconds(120);
        let browser_end = now - Duration::seconds(60);

        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'code', 'Editor')",
            params![app_start.to_rfc3339(), app_end.to_rfc3339()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'YouTube')",
            params![browser_start.to_rfc3339(), browser_end.to_rfc3339()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO domain_intervals(started_at, ended_at, browser_app_id, domain, url, source_profile)
             VALUES (?1, ?2, 'brave-browser', 'youtube.com', 'https://youtube.com/', 'Default')",
            params![browser_start.to_rfc3339(), browser_end.to_rfc3339()],
        )
        .unwrap();

        let status = build_status(&db, &config, true).unwrap();
        assert_eq!(status.watch_seconds, 120);
        assert!(status.tracked_seconds >= 0);
    }

    #[test]
    fn status_caps_open_intervals_at_idle_threshold() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config {
            idle_after_secs: 5,
            ..Default::default()
        };
        let now = Utc::now();
        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![(now - Duration::seconds(10)).to_rfc3339()],
        )
        .unwrap();

        let status = build_status(&db, &config, true).unwrap();
        assert_eq!(status.watch_seconds, 5);
    }

    fn unique_temp_db_path(label: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!(
            "{label}-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        path
    }

    // ----- focus source tests -----

    #[test]
    fn auto_detect_returns_niri_when_niri_socket_set() {
        // Set NIRI_SOCKET; also set HYPRLAND to prove niri wins.
        env::set_var("NIRI_SOCKET", "/run/user/1000/niri.sock");
        env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "some-sig");
        let result = focus::auto_detect();
        env::remove_var("NIRI_SOCKET");
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert_eq!(result, "niri");
    }

    #[test]
    fn auto_detect_returns_hyprland_when_signature_set_and_no_niri() {
        env::remove_var("NIRI_SOCKET");
        env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "some-sig");
        env::remove_var("SWAYSOCK");
        let result = focus::auto_detect();
        env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert_eq!(result, "hyprland");
    }

    #[test]
    fn hyprland_is_focus_event_line_matches_activewindow() {
        use crate::focus::hyprland::is_focus_event_line;
        assert!(is_focus_event_line("activewindow>>title,class"));
        assert!(is_focus_event_line("activewindowv2>>deadbeef"));
        assert!(!is_focus_event_line("openwindow>>address"));
        assert!(!is_focus_event_line(""));
    }

    #[test]
    fn hyprland_is_focus_event_line_does_not_panic_on_empty() {
        use crate::focus::hyprland::is_focus_event_line;
        assert!(!is_focus_event_line(""));
        assert!(!is_focus_event_line("   "));
    }
}
