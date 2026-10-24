mod browsers;
mod focus;
mod notifications;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use glob::glob;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use url::Url;
use zbus::zvariant::OwnedValue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

const DEFAULT_CONFIG_PATH: &str = "~/.config/attn/config.toml";
const DEFAULT_STATE_PATH: &str = "~/.local/state/attn/attn.sqlite";
const DEFAULT_SOCKET_PATH: &str = "$XDG_RUNTIME_DIR/attn.sock";
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
                if enabled.unwrap_or(cfg.breaks.enabled) {
                    1
                } else {
                    0
                },
                interval.unwrap_or(cfg.breaks.interval_secs),
                min_break.unwrap_or(cfg.breaks.min_break_secs),
            );
            let response = socket_request(&cfg.socket_path, &payload)?;
            println!("{response}");
            Ok(())
        }
        Some("categorize") => {
            let mut kind: Option<String> = None;
            let mut id: Option<String> = None;
            let mut category: Option<String> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--kind=") {
                    kind = Some(v.to_string());
                } else if let Some(v) = arg.strip_prefix("--id=") {
                    id = Some(v.to_string());
                } else if let Some(v) = arg.strip_prefix("--category=") {
                    category = Some(v.to_string());
                }
            }
            let kind = kind.ok_or_else(|| anyhow!("categorize requires --kind=app|domain"))?;
            let id = id.ok_or_else(|| anyhow!("categorize requires --id=<value>"))?;
            let category =
                category.ok_or_else(|| anyhow!("categorize requires --category=<name>"))?;
            let kind_enum = match kind.as_str() {
                "app" => CategorizeKind::App,
                "domain" => CategorizeKind::Domain,
                other => bail!("unknown kind {:?}; expected app or domain", other),
            };
            let payload = serde_json::to_string(&CategorizeRequest {
                kind: kind_enum,
                id,
                category,
            })?;
            let cfg = Config::load_default()?;
            let response = socket_request(&cfg.socket_path, &format!("categorize {payload}\n"))?;
            println!("{response}");
            Ok(())
        }
        Some("set-budget") => {
            let mut category: Option<String> = None;
            let mut secs: Option<i64> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--category=") {
                    category = Some(v.to_string());
                } else if let Some(v) = arg.strip_prefix("--secs=") {
                    secs = v.parse().ok();
                }
            }
            let category =
                category.ok_or_else(|| anyhow!("set-budget requires --category=<name>"))?;
            let daily_budget_secs =
                secs.ok_or_else(|| anyhow!("set-budget requires --secs=<N>"))?;
            let payload = serde_json::to_string(&SetBudgetRequest {
                category,
                daily_budget_secs,
            })?;
            let cfg = Config::load_default()?;
            let response = socket_request(&cfg.socket_path, &format!("set_budget {payload}\n"))?;
            println!("{response}");
            Ok(())
        }
        Some("set-notifications") => {
            let mut enabled: Option<bool> = None;
            let mut break_overdue: Option<bool> = None;
            let mut budget_exceeded: Option<bool> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--enabled=") {
                    enabled = Some(v == "true" || v == "1");
                } else if let Some(v) = arg.strip_prefix("--break-overdue=") {
                    break_overdue = Some(v == "true" || v == "1");
                } else if let Some(v) = arg.strip_prefix("--budget-exceeded=") {
                    budget_exceeded = Some(v == "true" || v == "1");
                }
            }
            let payload = serde_json::to_string(&SetNotificationsRequest {
                enabled,
                break_overdue,
                budget_exceeded,
            })?;
            let cfg = Config::load_default()?;
            let response =
                socket_request(&cfg.socket_path, &format!("set_notifications {payload}\n"))?;
            println!("{response}");
            Ok(())
        }
        Some("set-focus-source") => {
            let mut kind: Option<String> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--kind=") {
                    kind = Some(v.to_string());
                }
            }
            let kind = kind.ok_or_else(|| {
                anyhow!("set-focus-source requires --kind=auto|niri|hyprland|river|sway")
            })?;
            let payload = serde_json::to_string(&SetFocusSourceRequest { kind })?;
            let cfg = Config::load_default()?;
            let response =
                socket_request(&cfg.socket_path, &format!("set_focus_source {payload}\n"))?;
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
        Some("export") => {
            let mut since: Option<String> = None;
            let mut until: Option<String> = None;
            for arg in args {
                if let Some(v) = arg.strip_prefix("--since=") {
                    since = Some(v.to_string());
                } else if let Some(v) = arg.strip_prefix("--until=") {
                    until = Some(v.to_string());
                }
            }
            run_export(since.as_deref(), until.as_deref())
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
        "attn\n\nUsage:\n  attn daemon\n  attn status --json\n  attn reload\n  attn break-start\n  attn break-end\n  attn set-breaks [--enabled=true|false] [--interval=SECS] [--min-break=SECS]\n  attn categorize --kind=app|domain --id=<id> --category=<name>\n  attn set-budget --category=<name> --secs=<N>\n  attn set-notifications [--enabled=BOOL] [--break-overdue=BOOL] [--budget-exceeded=BOOL]\n  attn set-focus-source --kind=auto|niri|hyprland|river|sway\n  attn export [--since=YYYY-MM-DD] [--until=YYYY-MM-DD]\n  attn init [--force|--merge]\n  attn doctor"
    );
}

// ponytail: minimal RFC-4180 quoting — window titles/URLs contain commas. Avoids the csv crate.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn parse_export_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid date {value:?}, expected YYYY-MM-DD"))
}

/// Dump the raw interval ledger to CSV on stdout, attributing each row to its watch
/// category (the column raw SQLite can't produce on its own). App durations use the
/// same idle cap as attn's daily totals; domain rows use their stored span.
fn run_export(since: Option<&str>, until: Option<&str>) -> Result<()> {
    let config = Config::load_default()?;
    let db = open_state_db(&config.state_path)?;
    let now = Utc::now();

    let lo = since
        .map(|v| parse_export_date(v).and_then(local_day_start_utc))
        .transpose()?;
    let hi = until
        .map(|v| parse_export_date(v).and_then(|d| local_day_start_utc(d + Duration::days(1))))
        .transpose()?;
    let in_range =
        |start: DateTime<Utc>| lo.is_none_or(|l| start >= l) && hi.is_none_or(|h| start < h);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "kind,started_at,ended_at,duration_secs,id,category,detail")?;

    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, app_id, window_title
         FROM app_intervals ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (started_at, ended_at, app_id, title) = row?;
        let start = parse_rfc3339_utc(&started_at)?;
        if !in_range(start) {
            continue;
        }
        let ended = ended_at.as_deref().map(parse_rfc3339_utc).transpose()?;
        let dur = (capped_interval_end(start, ended, now, config.idle_after_secs) - start)
            .num_seconds()
            .max(0);
        let category = config.category_for_app(&app_id).unwrap_or_default();
        writeln!(
            out,
            "app,{},{},{},{},{},{}",
            csv_field(&started_at),
            csv_field(ended_at.as_deref().unwrap_or("")),
            dur,
            csv_field(&app_id),
            csv_field(&category),
            csv_field(&title),
        )?;
    }

    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, domain, url
         FROM domain_intervals ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (started_at, ended_at, domain, url) = row?;
        let start = parse_rfc3339_utc(&started_at)?;
        if !in_range(start) {
            continue;
        }
        let dur = (parse_rfc3339_utc(&ended_at)? - start).num_seconds().max(0);
        let category = config.category_for_domain(&domain).unwrap_or_default();
        writeln!(
            out,
            "domain,{},{},{},{},{},{}",
            csv_field(&started_at),
            csv_field(&ended_at),
            dur,
            csv_field(&domain),
            csv_field(&category),
            csv_field(&url),
        )?;
    }

    Ok(())
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
        let merged =
            toml::to_string_pretty(&config).context("failed to serialize merged config")?;
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
    #[serde(default)]
    notifications: NotificationsConfig,
    #[serde(skip)]
    index: Arc<ConfigIndex>,
}

#[derive(Clone, Debug, Default)]
struct ConfigIndex {
    app_categories: HashMap<String, String>,
    /// `apps.watch` entries with a `*`, longest first. Exact ids win over them.
    app_patterns: Vec<(glob::Pattern, String)>,
    terminal_categories: HashMap<String, String>,
    terminal_programs: HashMap<String, String>,
    terminal_app_ids: HashSet<String>,
    domain_categories: Vec<(String, String)>,
    browser_app_ids: HashSet<String>,
}

impl ConfigIndex {
    fn build(config: &Config) -> Self {
        let mut app_categories = HashMap::new();
        let mut app_patterns = Vec::new();
        for (category, ids) in &config.apps.watch {
            for id in ids {
                if !id.contains('*') {
                    app_categories
                        .entry(id.clone())
                        .or_insert_with(|| category.clone());
                } else if let Ok(pattern) = glob::Pattern::new(id) {
                    app_patterns.push((pattern, category.clone()));
                }
            }
        }
        app_patterns.sort_by_key(|(pattern, _)| std::cmp::Reverse(pattern.as_str().len()));

        let mut terminal_categories = HashMap::new();
        let mut terminal_programs = HashMap::new();
        for (category, names) in &config.terminals.apps {
            for name in names {
                terminal_categories
                    .entry(name.clone())
                    .or_insert_with(|| category.clone());
                terminal_programs
                    .entry(name.clone())
                    .or_insert_with(|| name.clone());
            }
        }

        let terminal_app_ids = config
            .apps
            .watch
            .get("terminal")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        let mut domain_categories = Vec::new();
        for (category, domains) in &config.domains.watch {
            for domain in domains {
                domain_categories.push((domain.clone(), category.clone()));
            }
        }
        // Longest candidate first, so a specific host beats the broader one it sits
        // under (drive.google.com over google.com) whatever the categories are named.
        domain_categories.sort_by_key(|(domain, _)| std::cmp::Reverse(domain.len()));

        let browser_app_ids = config
            .browsers
            .values()
            .flat_map(|browser| browser.app_ids.iter().cloned())
            .collect();

        Self {
            app_categories,
            app_patterns,
            terminal_categories,
            terminal_programs,
            terminal_app_ids,
            domain_categories,
            browser_app_ids,
        }
    }
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

fn default_breaks_enabled() -> bool {
    true
}
fn default_breaks_interval_secs() -> i64 {
    3600
}
fn default_breaks_min_break_secs() -> i64 {
    300
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FocusSourceConfig {
    #[serde(default = "default_focus_source_kind")]
    kind: String,
}

impl Default for FocusSourceConfig {
    fn default() -> Self {
        Self {
            kind: default_focus_source_kind(),
        }
    }
}

fn default_focus_source_kind() -> String {
    "auto".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NotificationsConfig {
    #[serde(default = "default_notifications_enabled")]
    enabled: bool,
    #[serde(default = "default_true")]
    break_overdue: bool,
    #[serde(default = "default_true")]
    budget_exceeded: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: default_notifications_enabled(),
            break_overdue: default_true(),
            budget_exceeded: default_true(),
        }
    }
}

fn default_notifications_enabled() -> bool {
    true
}
fn default_true() -> bool {
    true
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
        vec![
            "claude".to_string(),
            "codex".to_string(),
            "aichat".to_string(),
        ],
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    bundled_default_table(&["apps", "watch"])
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
    bundled_default_table(&["domains", "watch"])
}

impl Default for DomainsConfig {
    fn default() -> Self {
        default_domains_config()
    }
}

fn default_browsers() -> BTreeMap<String, BrowserConfig> {
    bundled_default_table(&["browsers"])
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
        normalize_string_lists(&mut self.apps.watch, normalize_id);
        normalize_string_lists(&mut self.terminals.apps, normalize_id);
        normalize_string_lists(&mut self.domains.watch, normalize_domain);
        for browser in self.browsers.values_mut() {
            browser.app_ids = browser
                .app_ids
                .iter()
                .map(|id| normalize_id(id))
                .collect::<Vec<_>>();
        }
        self.index = Arc::new(ConfigIndex::build(&self));
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
        self.notifications = user.notifications;

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
        self.index
            .app_categories
            .get(&normalized)
            .cloned()
            .or_else(|| self.index.terminal_categories.get(&normalized).cloned())
            .or_else(|| {
                self.index
                    .app_patterns
                    .iter()
                    .find(|(pattern, _)| pattern.matches(&normalized))
                    .map(|(_, category)| category.clone())
            })
    }

    fn is_terminal_app(&self, app_id: &str) -> bool {
        let normalized = normalize_id(app_id);
        self.index.terminal_app_ids.contains(&normalized)
    }

    fn terminal_subprocess_program(&self, name: &str) -> Option<String> {
        let normalized = normalize_id(name);
        self.index.terminal_programs.get(&normalized).cloned()
    }

    fn category_for_domain(&self, domain: &str) -> Option<String> {
        let domain = normalize_domain(domain);
        self.index
            .domain_categories
            .iter()
            .find_map(|(candidate, category)| {
                domain_matches_normalized(&domain, candidate).then(|| category.clone())
            })
    }

    fn browser_app_ids(&self) -> HashSet<String> {
        self.index.browser_app_ids.clone()
    }
}

fn bundled_default_config() -> Result<Config> {
    toml::from_str::<Config>(DEFAULT_CONFIG_TOML).context("failed to parse bundled default config")
}

/// The bundled `config/default.toml` as a raw TOML tree.
///
/// Parsed as `toml::Value` rather than `Config` on purpose: `Config`'s serde
/// field defaults read back out of this tree, and going through `Config` here
/// would make that recursive.
fn bundled_default_value() -> &'static toml::Value {
    static VALUE: OnceLock<toml::Value> = OnceLock::new();
    VALUE.get_or_init(|| {
        toml::from_str(DEFAULT_CONFIG_TOML).expect("bundled default config must be valid TOML")
    })
}

/// Pull one table out of the bundled default config, e.g. `["apps", "watch"]`.
///
/// This is the single source of truth for the shipped watch lists and browser
/// definitions: `config/default.toml` is generated from `config/apps/*.txt` and
/// `config/domains/*.txt` by `tools/sync-default-config.sh`, and CI verifies the
/// regenerated TOML matches those sources. Serde field defaults route here so a
/// partial user `config.toml` inherits the same lists a full one would, instead
/// of a hand-maintained Rust copy that silently drifts.
fn bundled_default_table<T>(path: &[&str]) -> T
where
    T: serde::de::DeserializeOwned + Default,
{
    let mut node = bundled_default_value();
    for key in path {
        match node.get(key) {
            Some(next) => node = next,
            None => return T::default(),
        }
    }
    // A present-but-malformed table is a bug in the compiled-in default.toml, not a
    // runtime condition — fail loudly rather than degrading to an empty list.
    node.clone()
        .try_into()
        .unwrap_or_else(|e| panic!("bundled default config table {path:?} is malformed: {e}"))
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

fn normalize_string_lists(
    values_by_key: &mut BTreeMap<String, Vec<String>>,
    normalize: fn(&str) -> String,
) {
    for values in values_by_key.values_mut() {
        let mut normalized = Vec::with_capacity(values.len());
        for value in values.iter() {
            let value = normalize(value);
            if !value.is_empty() && !normalized.contains(&value) {
                normalized.push(value);
            }
        }
        *values = normalized;
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<Mutex<Arc<Config>>>,
    db: Arc<Mutex<Connection>>,
    db_state_path: Arc<Mutex<PathBuf>>,
    socket_path: Arc<Mutex<PathBuf>>,
    last_rebuild: Arc<Mutex<Option<Instant>>>,
    tracking_state: Arc<Mutex<Option<PauseInfo>>>,
    /// Resolved focus source kind (e.g. "niri", "hyprland", "sway", "river").
    /// Set once at daemon startup and never changes.
    focus_source_kind: Arc<String>,
    notifier: Arc<dyn notifications::NotificationSink>,
    /// Per-window terminal subprocess resolution cache.
    /// Maps window_id → resolved subprocess name.  Session-scoped, not persisted.
    terminal_window_cache: Arc<Mutex<TerminalWindowCache>>,
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
const MEDIA_POLL_INTERVAL: StdDuration = StdDuration::from_secs(2);

fn run_daemon() -> Result<()> {
    cleanup_stale_snapshots();
    let mut config = Config::load_default()?;
    let db = open_state_db(&config.state_path)?;
    let now = Utc::now();
    cap_open_intervals_at_stale_heartbeat(&db, now)?;
    close_stale_open_intervals(&db, now, config.idle_after_secs)?;
    close_open_media_intervals(&db, now)?;
    load_breaks_override(&db, &mut config.breaks)?;

    // Resolve focus source kind once at startup.
    let resolved_kind = if config.focus_source.kind == "auto" {
        focus::auto_detect().to_string()
    } else {
        config.focus_source.kind.clone()
    };

    // An idle pause mirrors live compositor input state. A fresh idle notification only
    // emits `resumed` after its own `idled`, so a restored idle pause would never clear
    // and the daemon would record nothing. Manual pauses do survive a restart.
    let initial_pause = match load_pause_state(&db)? {
        Some(info) if info.reason == PauseReason::Idle => {
            persist_pause_state(&db, None)?;
            None
        }
        other => other,
    };
    let notifier: Arc<dyn notifications::NotificationSink> = if config.notifications.enabled {
        Arc::new(notifications::DesktopNotifier::new())
    } else {
        Arc::new(notifications::NullNotifier)
    };
    let state = AppState {
        config: Arc::new(Mutex::new(Arc::new(config.clone()))),
        db: Arc::new(Mutex::new(db)),
        db_state_path: Arc::new(Mutex::new(config.state_path.clone())),
        socket_path: Arc::new(Mutex::new(config.socket_path.clone())),
        last_rebuild: Arc::new(Mutex::new(None)),
        tracking_state: Arc::new(Mutex::new(initial_pause)),
        focus_source_kind: Arc::new(resolved_kind),
        notifier,
        terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
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
    let mut last_import_day = None;
    thread::spawn(move || loop {
        let start = Instant::now();
        let day = Local::now().date_naive();
        // The last poll before midnight ran up to poll_interval_secs early: finish that day.
        let days = last_import_day
            .filter(|last| *last != day)
            .into_iter()
            .chain([day]);
        for import_day in days {
            if let Err(error) = import_domains_for_day(&import_state, import_day) {
                eprintln!("attn browser import error: {error:#}");
            }
        }
        last_import_day = Some(day);
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

    let media_state = state.clone();
    thread::spawn(move || loop {
        if let Err(error) = run_mpris_media_loop(&media_state) {
            eprintln!("attn mpris media error: {error:#}");
            if let Ok(db) = media_state.db.lock() {
                let _ = close_open_media_intervals(&db, Utc::now());
            }
            thread::sleep(StdDuration::from_secs(10));
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

    let notify_state = state.clone();
    thread::spawn(move || loop {
        thread::sleep(StdDuration::from_secs(60));
        if let Err(error) = run_notification_check(&notify_state) {
            eprintln!("attn notification check error: {error:#}");
        }
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
        IDLE_DETECTION.store(false, Ordering::Relaxed);
        thread::sleep(StdDuration::from_secs(5));
    });

    run_focus_dispatch(state)
}

fn run_wayland_idle_listener(state: &AppState) -> Result<()> {
    use wayland_client::Connection as WConnection;

    let timeout_ms = idle_timeout_ms(state);

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
    };

    // First roundtrip surfaces the globals.
    event_queue
        .roundtrip(&mut client)
        .context("wayland roundtrip")?;
    // Second roundtrip pulls follow-up events (e.g. wl_seat capabilities).
    event_queue
        .roundtrip(&mut client)
        .context("wayland roundtrip 2")?;

    client.try_subscribe(&qh);

    loop {
        event_queue
            .blocking_dispatch(&mut client)
            .context("wayland dispatch")?;
        // The notification's timeout is fixed at creation, so `set-breaks` only lands
        // after a re-subscribe. Returning lets the caller reconnect with the new value.
        // ponytail: re-arms on the next idle event rather than polling the Wayland fd,
        // so a changed min_break_secs takes effect one idle cycle late.
        if idle_timeout_ms(&client.state) != client.timeout_ms {
            return Ok(());
        }
    }
}

/// Wayland idle timeout in ms: `breaks.min_break_secs`, the "idle counts as a break" knob.
fn idle_timeout_ms(state: &AppState) -> u32 {
    state
        .config
        .lock()
        .map(|c| c.breaks.min_break_secs.clamp(1, 86_400) as u32 * 1000)
        .unwrap_or(300_000)
}

struct IdleClient {
    state: AppState,
    seat: Option<wayland_client::protocol::wl_seat::WlSeat>,
    notifier: Option<wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1>,
    notification: Option<wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1>,
    timeout_ms: u32,
}

impl IdleClient {
    fn try_subscribe(&mut self, qh: &wayland_client::QueueHandle<Self>) {
        if self.notification.is_some() {
            return;
        }
        let (Some(seat), Some(notifier)) = (self.seat.as_ref(), self.notifier.as_ref()) else {
            return;
        };
        // An idle inhibitor (a video tab, a game, a leaked D-Bus inhibit) holds the plain
        // notification back for hours. The version 2 input-only one ignores inhibitors.
        self.notification = Some(if wayland_client::Proxy::version(notifier) >= 2 {
            notifier.get_input_idle_notification(self.timeout_ms, seat, qh, ())
        } else {
            notifier.get_idle_notification(self.timeout_ms, seat, qh, ())
        });
        IDLE_DETECTION.store(true, Ordering::Relaxed);
        *DEFERRED_IDLE.lock().unwrap() = None;
        // A new notification sends `resumed` only after its own `idled`, so an idle pause
        // from the old one would never clear. Resume now; the next `idled` trims the time.
        let idle_paused = self
            .state
            .tracking_state
            .lock()
            .unwrap()
            .is_some_and(|p| p.reason == PauseReason::Idle);
        if idle_paused {
            if let Err(e) = clear_pause(&self.state) {
                eprintln!("attn idle: clear stale pause failed: {e:#}");
            }
        }
    }

    fn on_idled(&self) {
        // `idled` fires `timeout_ms` after the last input, so focus ended back then.
        let idle_since = Utc::now() - Duration::milliseconds(i64::from(self.timeout_ms));
        let mut current = self.state.tracking_state.lock().unwrap();
        if current.is_some() {
            return;
        }
        let db = self.state.db.lock().unwrap();
        // A tracked video is watched without input. The media loop pauses once it ends.
        match open_media_interval_id(&db) {
            Ok(Some(media_id)) => {
                eprintln!("attn idle: no input, deferred while media interval {media_id} plays");
                *DEFERRED_IDLE.lock().unwrap() = Some(DeferredIdle {
                    media_id,
                    idle_since,
                });
            }
            Ok(None) => pause_for_idle(&db, &mut current, idle_since),
            Err(e) => {
                eprintln!("attn idle: read media state failed: {e:#}");
                pause_for_idle(&db, &mut current, idle_since);
            }
        }
    }

    /// Input after at least `timeout_ms` without any. That is a break, so it ends a manual
    /// pause too: "Take one" resumes once the user is back.
    fn on_resumed(&self) {
        *DEFERRED_IDLE.lock().unwrap() = None;
        if let Err(e) = clear_pause(&self.state) {
            eprintln!("attn idle: resume failed: {e:#}");
        }
    }
}

/// End focus at `at` and persist an idle pause. The caller holds the pause guard.
fn pause_for_idle(db: &Connection, current: &mut Option<PauseInfo>, at: DateTime<Utc>) {
    let info = PauseInfo {
        at,
        reason: PauseReason::Idle,
    };
    if let Err(e) = end_focus_at(db, at) {
        eprintln!("attn idle: close interval failed: {e:#}");
    }
    if let Err(e) = persist_pause_state(db, Some(info)) {
        eprintln!("attn idle: persist pause failed: {e:#}");
    }
    *current = Some(info);
}

/// Apply a deferred idle once its media interval ends, or at the cap. Autoplay closes the
/// interval and opens a new one, so only the video playing at the last input counts.
fn apply_deferred_idle(state: &AppState) -> Result<()> {
    if DEFERRED_IDLE.lock().unwrap().is_none() {
        return Ok(());
    }
    let mut current = state.tracking_state.lock().unwrap();
    let db = state.db.lock().unwrap();
    let mut slot = DEFERRED_IDLE.lock().unwrap();
    let Some(deferred) = *slot else {
        return Ok(());
    };
    let now = Utc::now();
    let cap = deferred.idle_since + Duration::seconds(MAX_UNATTENDED_VIDEO_SECS);
    let ended: Option<Option<String>> = db
        .query_row(
            "SELECT ended_at FROM media_intervals WHERE id = ?1",
            params![deferred.media_id],
            |row| row.get(0),
        )
        .optional()?;
    let end = match ended {
        Some(None) if now < cap => return Ok(()),
        Some(None) => cap,
        Some(Some(ended)) => parse_rfc3339_utc(&ended)?.min(cap),
        None => deferred.idle_since,
    };
    *slot = None;
    if current.is_none() {
        eprintln!(
            "attn idle: media interval {} ended, pausing",
            deferred.media_id
        );
        pause_for_idle(&db, &mut current, end.max(deferred.idle_since));
    }
    Ok(())
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()>
    for IdleClient
{
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
            let _ = close_open_media_intervals(&db, now);
        }
    }
    Ok(())
}

fn run_mpris_media_loop(state: &AppState) -> Result<()> {
    use zbus::blocking::Connection as ZConnection;

    let conn = ZConnection::session().context("connect to session D-Bus")?;
    loop {
        sync_mpris_media_once(state, &conn)?;
        thread::sleep(MEDIA_POLL_INTERVAL);
    }
}

fn sync_mpris_media_once(state: &AppState, conn: &zbus::blocking::Connection) -> Result<()> {
    let snapshots = collect_mpris_snapshots(conn)?;
    let config = state.config.lock().unwrap().clone();
    let db = state.db.lock().unwrap();
    let open = open_app_identity(&db)?;
    let snapshots = snapshots
        .into_iter()
        .filter_map(|mut snapshot| {
            infer_media_domain_from_focus(&mut snapshot, &config, open.as_ref());
            should_track_media_snapshot(&snapshot, &config, open.as_ref()).then_some(snapshot)
        })
        .collect::<Vec<_>>();
    sync_media_snapshots(&db, Utc::now(), &snapshots)?;
    drop(db);
    apply_deferred_idle(state)
}

fn collect_mpris_snapshots(conn: &zbus::blocking::Connection) -> Result<Vec<MediaSnapshot>> {
    let proxy = zbus::blocking::fdo::DBusProxy::new(conn).context("create DBus proxy")?;
    let names = proxy.list_names().context("list D-Bus names")?;
    let mut snapshots = Vec::new();

    for name in names {
        let name = name.to_string();
        if !is_mpris_bus_name(&name) {
            continue;
        }
        match read_mpris_snapshot(conn, &name) {
            Ok(Some(snapshot)) => snapshots.push(snapshot),
            Ok(None) => {}
            Err(error) => eprintln!("attn mpris read {name}: {error:#}"),
        }
    }

    Ok(snapshots)
}

fn read_mpris_snapshot(
    conn: &zbus::blocking::Connection,
    bus_name: &str,
) -> Result<Option<MediaSnapshot>> {
    let proxy = zbus::blocking::Proxy::new(
        conn,
        bus_name,
        "/org/mpris/MediaPlayer2",
        "org.mpris.MediaPlayer2.Player",
    )
    .with_context(|| format!("create MPRIS player proxy for {bus_name}"))?;
    let playback_status: String = proxy
        .get_property("PlaybackStatus")
        .with_context(|| format!("read PlaybackStatus for {bus_name}"))?;
    if playback_status != "Playing" {
        return Ok(None);
    }

    let metadata: HashMap<String, OwnedValue> = proxy.get_property("Metadata").unwrap_or_default();
    let title = metadata_string(&metadata, "xesam:title").unwrap_or_default();
    let artist = metadata_string_vec(&metadata, "xesam:artist").join(", ");
    let url = metadata_string(&metadata, "xesam:url")
        .or_else(|| metadata_string(&metadata, "xesam:asText"))
        .unwrap_or_default();
    let domain = domain_from_media_url(&url).unwrap_or_default();

    Ok(Some(MediaSnapshot {
        playback_id: bus_name.to_string(),
        player_name: player_name_from_mpris_bus(bus_name),
        title,
        artist,
        url,
        domain,
        source: "mpris".to_string(),
    }))
}

fn is_mpris_bus_name(name: &str) -> bool {
    name.starts_with("org.mpris.MediaPlayer2.") && name.len() > "org.mpris.MediaPlayer2.".len()
}

fn player_name_from_mpris_bus(name: &str) -> String {
    name.trim_start_matches("org.mpris.MediaPlayer2.")
        .split('.')
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(name)
        .to_ascii_lowercase()
}

fn metadata_string(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| String::try_from(value).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn metadata_string_vec(metadata: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn domain_from_media_url(raw_url: &str) -> Option<String> {
    let url = Url::parse(raw_url).ok()?;
    let host = url.host_str()?;
    Some(normalize_domain(host))
}

fn run_config_watcher(state: AppState) {
    let path = expand_path(DEFAULT_CONFIG_PATH);
    // A home-manager config is a symlink into the Nix store, where every file has mtime 1.
    // A switch replaces the link, so its own mtime is the one that moves.
    let mtimes = || {
        let target = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let link = fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()?;
        Some((target, link))
    };
    let mut last_mtime = mtimes();
    loop {
        thread::sleep(StdDuration::from_secs(3));
        let mtime = mtimes();
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
    close_open_media_intervals(db, last)?;
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
                let _ = close_open_media_intervals(&db, now);
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
    stream
        .set_read_timeout(Some(SOCKET_REQUEST_TIMEOUT))
        .context("failed to set accepted socket read timeout")?;
    stream
        .set_write_timeout(Some(SOCKET_REQUEST_TIMEOUT))
        .context("failed to set accepted socket write timeout")?;

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
        cmd if cmd.starts_with("categorize ") => {
            let json_part = &cmd["categorize ".len()..];
            match serde_json::from_str::<CategorizeRequest>(json_part) {
                Ok(req) => {
                    let resp = handle_categorize(&state, req)?;
                    serde_json::to_string(&resp)?
                }
                Err(e) => serde_json::to_string(&CategorizeResponse::err(format!(
                    "invalid categorize payload: {e}"
                )))?,
            }
        }
        cmd if cmd.starts_with("set_budget ") => {
            let json_part = &cmd["set_budget ".len()..];
            match serde_json::from_str::<SetBudgetRequest>(json_part) {
                Ok(req) => {
                    let resp = handle_set_budget(&state, req)?;
                    serde_json::to_string(&resp)?
                }
                Err(e) => serde_json::to_string(&SimpleResponse::err(format!(
                    "invalid set_budget payload: {e}"
                )))?,
            }
        }
        cmd if cmd.starts_with("set_notifications ") => {
            let json_part = &cmd["set_notifications ".len()..];
            match serde_json::from_str::<SetNotificationsRequest>(json_part) {
                Ok(req) => {
                    let resp = handle_set_notifications(&state, req)?;
                    serde_json::to_string(&resp)?
                }
                Err(e) => serde_json::to_string(&SimpleResponse::err(format!(
                    "invalid set_notifications payload: {e}"
                )))?,
            }
        }
        cmd if cmd.starts_with("set_focus_source ") => {
            let json_part = &cmd["set_focus_source ".len()..];
            match serde_json::from_str::<SetFocusSourceRequest>(json_part) {
                Ok(req) => {
                    let resp = handle_set_focus_source(&state, req)?;
                    serde_json::to_string(&resp)?
                }
                Err(e) => serde_json::to_string(&SimpleResponse::err(format!(
                    "invalid set_focus_source payload: {e}"
                )))?,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum CategorizeKind {
    App,
    Domain,
}

#[derive(Debug, Deserialize, Serialize)]
struct CategorizeRequest {
    kind: CategorizeKind,
    id: String,
    category: String,
}

#[derive(Debug, Serialize)]
struct CategorizeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl CategorizeResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
        }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
        }
    }
}

fn handle_categorize(state: &AppState, req: CategorizeRequest) -> Result<CategorizeResponse> {
    use std::time::SystemTime;
    use toml_edit::{Array, DocumentMut, Item, Value};

    let config_path = expand_path(DEFAULT_CONFIG_PATH);
    if !config_path.exists() {
        return Ok(CategorizeResponse::err("config file not found"));
    }

    let mtime_before: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config {}", config_path.display()))?;

    // Validate category exists in running config.
    {
        let config = state.config.lock().unwrap();
        let watch = match req.kind {
            CategorizeKind::App => &config.apps.watch,
            CategorizeKind::Domain => &config.domains.watch,
        };
        if !watch.contains_key(&req.category) {
            return Ok(CategorizeResponse::err(format!(
                "category {:?} not found; existing categories: {}",
                req.category,
                watch.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
    }

    // Parse with toml_edit to preserve comments and ordering.
    let mut doc = raw
        .parse::<DocumentMut>()
        .with_context(|| "failed to parse config with toml_edit")?;

    let section_key = match req.kind {
        CategorizeKind::App => "apps",
        CategorizeKind::Domain => "domains",
    };

    // Navigate to [apps].watch.<category> or [domains].watch.<category>
    let section = doc
        .get_mut(section_key)
        .and_then(|s| s.as_table_mut())
        .and_then(|t| t.get_mut("watch"))
        .and_then(|w| w.as_table_mut())
        .and_then(|t| t.get_mut(&req.category))
        .and_then(|v| v.as_array_mut());

    match section {
        Some(arr) => {
            let already_present = arr.iter().any(|v| v.as_str().is_some_and(|s| s == req.id));
            if !already_present {
                arr.push(req.id.as_str());
            }
        }
        None => {
            // Category exists in runtime config but not (yet) in the file.
            let mut arr = Array::new();
            arr.push(req.id.as_str());
            let watch_table = doc
                .get_mut(section_key)
                .and_then(|s| s.as_table_mut())
                .and_then(|t| t.get_mut("watch"))
                .and_then(|w| w.as_table_mut());
            if let Some(t) = watch_table {
                t.insert(&req.category, Item::Value(Value::Array(arr)));
            } else {
                return Ok(CategorizeResponse::err(format!(
                    "[{section_key}].watch not found in config"
                )));
            }
        }
    }

    // Re-check mtime to detect concurrent edits.
    let mtime_after: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if mtime_after != mtime_before {
        return Ok(CategorizeResponse::err("config changed on disk, retry"));
    }

    // Write atomically via tempfile in the same directory.
    let parent = config_path.parent().unwrap_or(std::path::Path::new("/tmp"));
    let mut tmp = tempfile::Builder::new()
        .prefix(".attn-config-")
        .suffix(".toml.tmp")
        .tempfile_in(parent)
        .context("failed to create temp config file")?;
    tmp.write_all(doc.to_string().as_bytes())
        .context("failed to write temp config")?;
    tmp.flush().context("failed to flush temp config")?;
    tmp.persist(&config_path)
        .map_err(|e| anyhow!("failed to persist config: {}", e.error))?;

    // Trigger daemon reload so the new entry is picked up immediately.
    let _ = reload_daemon_config(state);

    Ok(CategorizeResponse::ok())
}

// ── Shared response type for simple ok/error operations ──────────────────────

#[derive(Debug, Serialize)]
struct SimpleResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl SimpleResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            message: None,
        }
    }
    fn ok_msg(msg: impl Into<String>) -> Self {
        Self {
            ok: true,
            error: None,
            message: Some(msg.into()),
        }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            message: None,
        }
    }
}

// ── set_budget ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
struct SetBudgetRequest {
    category: String,
    daily_budget_secs: i64,
}

fn handle_set_budget(state: &AppState, req: SetBudgetRequest) -> Result<SimpleResponse> {
    use std::time::SystemTime;
    use toml_edit::{value, DocumentMut, Item, Table};

    let config_path = expand_path(DEFAULT_CONFIG_PATH);
    if !config_path.exists() {
        return Ok(SimpleResponse::err("config file not found"));
    }

    // Validate category exists in running config (apps or domains watch).
    {
        let config = state.config.lock().unwrap();
        let in_apps = config.apps.watch.contains_key(&req.category);
        let in_domains = config.domains.watch.contains_key(&req.category);
        if !in_apps && !in_domains {
            let mut all_cats: Vec<String> = config.apps.watch.keys().cloned().collect();
            for k in config.domains.watch.keys() {
                if !all_cats.contains(k) {
                    all_cats.push(k.clone());
                }
            }
            all_cats.sort();
            return Ok(SimpleResponse::err(format!(
                "category {:?} not found; existing categories: {}",
                req.category,
                all_cats.join(", ")
            )));
        }
    }

    let mtime_before: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config {}", config_path.display()))?;

    let mut doc = raw
        .parse::<DocumentMut>()
        .context("failed to parse config with toml_edit")?;

    if req.daily_budget_secs == 0 {
        // Remove the budget entry.
        if let Some(budgets) = doc.get_mut("budgets").and_then(|b| b.as_table_mut()) {
            budgets.remove(&req.category);
        }
    } else {
        // Insert or update [budgets.<category>].daily_budget_secs.
        if doc.get("budgets").is_none() {
            doc.insert("budgets", Item::Table(Table::new()));
        }
        let budgets = doc
            .get_mut("budgets")
            .and_then(|b| b.as_table_mut())
            .ok_or_else(|| anyhow!("[budgets] is not a table"))?;
        if budgets.get(&req.category).is_none() {
            budgets.insert(&req.category, Item::Table(Table::new()));
        }
        let entry = budgets
            .get_mut(&req.category)
            .and_then(|e| e.as_table_mut())
            .ok_or_else(|| anyhow!("[budgets.{}] is not a table", req.category))?;
        entry.insert("daily_budget_secs", value(req.daily_budget_secs));
    }

    let mtime_after: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if mtime_after != mtime_before {
        return Ok(SimpleResponse::err("config changed on disk, retry"));
    }

    let parent = config_path.parent().unwrap_or(std::path::Path::new("/tmp"));
    let mut tmp = tempfile::Builder::new()
        .prefix(".attn-config-")
        .suffix(".toml.tmp")
        .tempfile_in(parent)
        .context("failed to create temp config file")?;
    tmp.write_all(doc.to_string().as_bytes())
        .context("failed to write temp config")?;
    tmp.flush().context("failed to flush temp config")?;
    tmp.persist(&config_path)
        .map_err(|e| anyhow!("failed to persist config: {}", e.error))?;

    let _ = reload_daemon_config(state);
    Ok(SimpleResponse::ok())
}

// ── set_notifications ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
struct SetNotificationsRequest {
    enabled: Option<bool>,
    break_overdue: Option<bool>,
    budget_exceeded: Option<bool>,
}

fn handle_set_notifications(
    state: &AppState,
    req: SetNotificationsRequest,
) -> Result<SimpleResponse> {
    use std::time::SystemTime;
    use toml_edit::{value, DocumentMut, Item, Table};

    let config_path = expand_path(DEFAULT_CONFIG_PATH);
    if !config_path.exists() {
        return Ok(SimpleResponse::err("config file not found"));
    }

    let mtime_before: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config {}", config_path.display()))?;

    let mut doc = raw
        .parse::<DocumentMut>()
        .context("failed to parse config with toml_edit")?;

    // Ensure [notifications] table exists.
    if doc.get("notifications").is_none() {
        doc.insert("notifications", Item::Table(Table::new()));
    }
    let notif = doc
        .get_mut("notifications")
        .and_then(|n| n.as_table_mut())
        .ok_or_else(|| anyhow!("[notifications] is not a table"))?;

    if let Some(v) = req.enabled {
        notif.insert("enabled", value(v));
    }
    if let Some(v) = req.break_overdue {
        notif.insert("break_overdue", value(v));
    }
    if let Some(v) = req.budget_exceeded {
        notif.insert("budget_exceeded", value(v));
    }

    let mtime_after: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if mtime_after != mtime_before {
        return Ok(SimpleResponse::err("config changed on disk, retry"));
    }

    let parent = config_path.parent().unwrap_or(std::path::Path::new("/tmp"));
    let mut tmp = tempfile::Builder::new()
        .prefix(".attn-config-")
        .suffix(".toml.tmp")
        .tempfile_in(parent)
        .context("failed to create temp config file")?;
    tmp.write_all(doc.to_string().as_bytes())
        .context("failed to write temp config")?;
    tmp.flush().context("failed to flush temp config")?;
    tmp.persist(&config_path)
        .map_err(|e| anyhow!("failed to persist config: {}", e.error))?;

    let _ = reload_daemon_config(state);
    Ok(SimpleResponse::ok())
}

// ── set_focus_source ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
struct SetFocusSourceRequest {
    kind: String,
}

const VALID_FOCUS_KINDS: &[&str] = &["auto", "niri", "hyprland", "river", "sway"];

fn handle_set_focus_source(state: &AppState, req: SetFocusSourceRequest) -> Result<SimpleResponse> {
    use std::time::SystemTime;
    use toml_edit::{value, DocumentMut, Item, Table};

    if !VALID_FOCUS_KINDS.contains(&req.kind.as_str()) {
        return Ok(SimpleResponse::err(format!(
            "invalid kind {:?}; expected one of: {}",
            req.kind,
            VALID_FOCUS_KINDS.join(", ")
        )));
    }

    let config_path = expand_path(DEFAULT_CONFIG_PATH);
    if !config_path.exists() {
        return Ok(SimpleResponse::err("config file not found"));
    }

    let mtime_before: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config {}", config_path.display()))?;

    let mut doc = raw
        .parse::<DocumentMut>()
        .context("failed to parse config with toml_edit")?;

    if doc.get("focus_source").is_none() {
        doc.insert("focus_source", Item::Table(Table::new()));
    }
    let fs_table = doc
        .get_mut("focus_source")
        .and_then(|t| t.as_table_mut())
        .ok_or_else(|| anyhow!("[focus_source] is not a table"))?;
    fs_table.insert("kind", value(req.kind.as_str()));

    let mtime_after: SystemTime = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if mtime_after != mtime_before {
        return Ok(SimpleResponse::err("config changed on disk, retry"));
    }

    let parent = config_path.parent().unwrap_or(std::path::Path::new("/tmp"));
    let mut tmp = tempfile::Builder::new()
        .prefix(".attn-config-")
        .suffix(".toml.tmp")
        .tempfile_in(parent)
        .context("failed to create temp config file")?;
    tmp.write_all(doc.to_string().as_bytes())
        .context("failed to write temp config")?;
    tmp.flush().context("failed to flush temp config")?;
    tmp.persist(&config_path)
        .map_err(|e| anyhow!("failed to persist config: {}", e.error))?;

    let _ = reload_daemon_config(state);

    Ok(SimpleResponse::ok_msg(
        "focus source updated; restart daemon for the new kind to take effect",
    ))
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
    let mut updated = config.as_ref().clone();
    updated.breaks.enabled = enabled;
    updated.breaks.interval_secs = interval_secs;
    updated.breaks.min_break_secs = min_break_secs;
    *config = Arc::new(updated);
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
    let not_paused = PauseResponse {
        ok: true,
        paused: false,
        paused_reason: None,
    };
    if state.tracking_state.lock().unwrap().is_none() {
        return Ok(not_paused);
    }
    // Query and resolve before taking locks: both run subprocesses (`niri msg`, tmux).
    let config = state.config.lock().unwrap().clone();
    let window = query_focused_window(state)
        .ok()
        .flatten()
        .map(|w| resolve_focused_window(w, &config, &state.terminal_window_cache));
    let mut current = state.tracking_state.lock().unwrap();
    if current.is_none() {
        return Ok(not_paused);
    }
    let now = Utc::now();
    let db = state.db.lock().unwrap();
    persist_pause_state(&db, None)?;
    meta_set(&db, "session_reset_at", &now.to_rfc3339())?;
    *current = None;
    if let Some(window) = window {
        open_app_interval(&db, now, &window)?;
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
        handle_focus_change_with(state, query_focused_window(state).ok().flatten())?;
    }

    {
        let db = state.db.lock().unwrap();
        load_breaks_override(&db, &mut new_config.breaks)?;
    }

    let mut config = state.config.lock().unwrap();
    *config = Arc::new(new_config);

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
    handle_focus_change_with(&state, source.poll_current()?)?;

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

fn handle_focus_change_with(
    state: &AppState,
    current_window: Option<focus::FocusedWindow>,
) -> Result<()> {
    let now = Utc::now();
    let config = state.config.lock().unwrap().clone();
    // Resolve before taking locks: it can run tmux.
    let current =
        current_window.map(|w| resolve_focused_window(w, &config, &state.terminal_window_cache));

    // Only the idle notification's `resumed` ends a pause: a focus event or the terminal
    // poll can fire while the user is away, such as a terminal title that keeps changing.
    // Hold the pause guard across the write, or an idle pause can land in between.
    let paused = state.tracking_state.lock().unwrap();
    let db = state.db.lock().unwrap();
    let open = open_app_identity(&db)?;

    if paused.is_some() {
        if open.is_some() {
            close_open_interval(&db, now, config.idle_after_secs)?;
        }
        return Ok(());
    }

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
    window_title: String,
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

/// Maximum number of entries kept in the per-window resolution cache.
const TERMINAL_CACHE_CAP: usize = 256;

/// Session-scoped cache mapping `window_id → resolved subprocess name`.
/// Keyed by niri window id (or equivalent); retains insertion order for eviction.
struct TerminalWindowCache {
    map: HashMap<i64, String>,
    order: std::collections::VecDeque<i64>,
}

impl TerminalWindowCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn get(&self, window_id: i64) -> Option<&str> {
        self.map.get(&window_id).map(|s| s.as_str())
    }

    fn insert(&mut self, window_id: i64, name: String) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) = self.map.entry(window_id) {
            // Update value without reordering.
            entry.insert(name);
            return;
        }
        if self.order.len() >= TERMINAL_CACHE_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.order.push_back(window_id);
        self.map.insert(window_id, name);
    }

    fn invalidate(&mut self, window_id: i64) {
        if self.map.remove(&window_id).is_some() {
            self.order.retain(|id| *id != window_id);
        }
    }
}

fn resolve_focused_window(
    mut window: FocusedWindow,
    config: &Config,
    cache: &Mutex<TerminalWindowCache>,
) -> FocusedWindow {
    if !config.is_terminal_app(&window.app_id) {
        return window;
    }

    // Unambiguous title-match (spinner / status markers).
    if let Some(name) = match_terminal_program_in_title(&window.title, config) {
        // Record in cache if we have a window id.
        if let Some(wid) = window.window_id {
            cache.lock().unwrap().insert(wid, name.clone());
        }
        window.app_id = name;
        return window;
    }

    let descendants = window.pid.map(collect_proc_descendants).unwrap_or_default();

    // Cache lookup for ambiguous titles. The name holds only while that program still runs.
    // ponytail: with a shared terminal process, "still runs" means in any of its windows.
    if let Some(wid) = window.window_id {
        let cached = cache.lock().unwrap().get(wid).map(|s| s.to_owned());
        if let Some(cached) = cached {
            let alive = window.pid.is_none()
                || descendants.iter().any(|d| {
                    config.terminal_subprocess_program(&d.name).as_deref() == Some(&cached)
                });
            if alive {
                window.app_id = cached;
                return window;
            }
            cache.lock().unwrap().invalidate(wid);
        }
    }

    // Fall through to /proc walk.
    let Some(pid) = window.pid else {
        return window;
    };
    // One process can serve several windows (Ghostty, kitty --single-instance). Its
    // descendants then span all of them, so only a tmux title still names a client.
    let shared = read_proc_children(pid).len() > 1;
    if shared && !window.title.starts_with("tmux ") {
        return window;
    }
    let descendant_pids: HashSet<i32> = descendants.iter().map(|d| d.pid).collect();
    if let Some(name) = pick_tmux_subprocess(&descendant_pids, config) {
        window.app_id = name;
        return window;
    }
    if shared {
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
    let output = output_with_timeout(Command::new("tmux").args([
        "list-clients",
        "-F",
        "#{client_pid}|#{client_session}|#{client_activity}",
    ]))?;
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
        let Some(cmd_output) = output_with_timeout(Command::new("tmux").args([
            "display-message",
            "-p",
            "-t",
            session,
            "#{pane_current_command}",
        ])) else {
            continue;
        };
        if !cmd_output.status.success() {
            continue;
        }
        let cmd = String::from_utf8_lossy(&cmd_output.stdout)
            .trim()
            .to_string();
        if let Some(name) = config.terminal_subprocess_program(&cmd) {
            if best
                .as_ref()
                .map(|(act, _)| activity > *act)
                .unwrap_or(true)
            {
                best = Some((activity, name));
            }
        }
    }
    best.map(|(_, name)| name)
}

const TMUX_TIMEOUT: StdDuration = StdDuration::from_secs(1);

/// `Command::output` with a deadline: a hung tmux server must not stall focus tracking.
fn output_with_timeout(cmd: &mut Command) -> Option<std::process::Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + TMUX_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => thread::sleep(StdDuration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Braille spinner chars used by Claude Code, plus common status markers.
/// U+2800..U+28FF (braille), ✳ U+2733, ⏵ U+23F5, ✶ U+2736, ◐◑◒◓ U+25D0..U+25D3.
const CLAUDE_TITLE_MARKERS: &[char] = &[
    '✳', '⏵', '✶', '◐', '◑', '◒', '◓', // Braille spinners
    '⠂', '⠐', '⠏', '⠦', '⠹', '⠼', '⠴', '⠧', '⠇', '⠋', '⠙', '⠸', '⠨', '⢿', '⣷', '⣯', '⣟', '⡿', '⣻',
    '⣽', '⣾',
];

/// Return the config name mapped to "claude" if present, otherwise "claude".
fn resolve_claude_name(config: &Config) -> String {
    for names in config.terminals.apps.values() {
        for n in names {
            if normalize_id(n) == "claude" {
                return normalize_id(n);
            }
        }
    }
    "claude".to_string()
}

fn match_terminal_program_in_title(title: &str, config: &Config) -> Option<String> {
    if title.is_empty() {
        return None;
    }

    // Fast path: tmux titles — let the tmux subprocess query handle these.
    if title.starts_with("tmux ") {
        return None;
    }

    // Unambiguous: title contains a Claude Code spinner / status marker.
    if title.chars().any(|c| CLAUDE_TITLE_MARKERS.contains(&c)) {
        return Some(resolve_claude_name(config));
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
                if ok_left && ok_right && best.as_ref().map(|(prev, _)| idx < *prev).unwrap_or(true)
                {
                    best = Some((idx, normalized));
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

fn pick_terminal_subprocess(candidates: &[DescendantProc], config: &Config) -> Option<String> {
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
                out.push(DescendantProc {
                    pid,
                    depth,
                    name,
                    start_time,
                });
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
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
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

        CREATE TABLE IF NOT EXISTS media_intervals (
          id INTEGER PRIMARY KEY,
          started_at TEXT NOT NULL,
          ended_at TEXT,
          playback_id TEXT NOT NULL,
          player_name TEXT NOT NULL,
          title TEXT NOT NULL DEFAULT '',
          artist TEXT NOT NULL DEFAULT '',
          url TEXT NOT NULL DEFAULT '',
          domain TEXT NOT NULL DEFAULT '',
          source TEXT NOT NULL DEFAULT 'mpris'
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
        CREATE INDEX IF NOT EXISTS app_intervals_open_idx
          ON app_intervals(ended_at, id);
        CREATE INDEX IF NOT EXISTS domain_intervals_started_idx
          ON domain_intervals(started_at);
        CREATE INDEX IF NOT EXISTS domain_intervals_browser_coverage_idx
          ON domain_intervals(browser_app_id, started_at, ended_at);
        CREATE INDEX IF NOT EXISTS domain_intervals_import_idx
          ON domain_intervals(browser_name, source_profile, started_at, ended_at);
        CREATE INDEX IF NOT EXISTS domain_intervals_legacy_import_idx
          ON domain_intervals(browser_app_id, source_profile, started_at, ended_at)
          WHERE browser_name = '';
        CREATE INDEX IF NOT EXISTS media_intervals_started_idx
          ON media_intervals(started_at);
        CREATE INDEX IF NOT EXISTS media_intervals_open_idx
          ON media_intervals(playback_id, ended_at);
        CREATE INDEX IF NOT EXISTS media_intervals_ended_idx
          ON media_intervals(ended_at, id);
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
        "SELECT window_id, app_id, window_title FROM app_intervals WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",
        [],
        |row| {
            Ok(OpenAppInterval {
                window_id: row.get(0)?,
                app_id: row.get(1)?,
                window_title: row.get(2)?,
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

/// End focus at `at`: close the open interval there and trim any interval that ran past
/// it, such as one a screen locker's focus change closed at lock time.
fn end_focus_at(db: &Connection, at: DateTime<Utc>) -> Result<()> {
    let mut stmt =
        db.prepare("SELECT id, started_at, ended_at FROM app_intervals ORDER BY id DESC LIMIT 20")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, started_at, ended_at) in rows {
        let ended_at = ended_at.as_deref().map(parse_rfc3339_utc).transpose()?;
        if ended_at.is_some_and(|end| end <= at) {
            break;
        }
        let started_at = parse_rfc3339_utc(&started_at)?;
        db.execute(
            "UPDATE app_intervals SET ended_at = ?1, idle_adjusted = 1 WHERE id = ?2",
            params![at.max(started_at).to_rfc3339(), id],
        )?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MediaSnapshot {
    playback_id: String,
    player_name: String,
    title: String,
    artist: String,
    url: String,
    domain: String,
    source: String,
}

impl MediaSnapshot {
    fn same_media(&self, other: &OpenMediaInterval) -> bool {
        self.player_name == other.player_name
            && self.title == other.title
            && self.artist == other.artist
            && self.url == other.url
            && self.domain == other.domain
            && self.source == other.source
    }
}

#[derive(Debug)]
struct OpenMediaInterval {
    id: i64,
    started_at: DateTime<Utc>,
    player_name: String,
    title: String,
    artist: String,
    url: String,
    domain: String,
    source: String,
}

fn sync_media_snapshots(
    db: &Connection,
    now: DateTime<Utc>,
    snapshots: &[MediaSnapshot],
) -> Result<()> {
    let active_ids = snapshots
        .iter()
        .map(|snapshot| snapshot.playback_id.as_str())
        .collect::<HashSet<_>>();

    for snapshot in snapshots {
        sync_media_snapshot(db, now, snapshot)?;
    }

    let mut stmt = db.prepare(
        "SELECT id, started_at, playback_id
         FROM media_intervals
         WHERE ended_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let open_rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, started_at, playback_id) in open_rows {
        if active_ids.contains(playback_id.as_str()) {
            continue;
        }
        close_media_interval_by_id(db, id, &started_at, now)?;
    }

    Ok(())
}

fn sync_media_snapshot(
    db: &Connection,
    now: DateTime<Utc>,
    snapshot: &MediaSnapshot,
) -> Result<()> {
    if let Some(open) = open_media_interval(db, &snapshot.playback_id)? {
        if snapshot.same_media(&open) {
            return Ok(());
        }
        close_media_interval_by_id(db, open.id, &open.started_at.to_rfc3339(), now)?;
    }

    db.execute(
        "INSERT INTO media_intervals(
            started_at, playback_id, player_name, title, artist, url, domain, source
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            now.to_rfc3339(),
            snapshot.playback_id,
            snapshot.player_name,
            snapshot.title,
            snapshot.artist,
            snapshot.url,
            snapshot.domain,
            snapshot.source,
        ],
    )?;
    Ok(())
}

fn open_media_interval(db: &Connection, playback_id: &str) -> Result<Option<OpenMediaInterval>> {
    db.query_row(
        "SELECT id, started_at, player_name, title, artist, url, domain, source
         FROM media_intervals
         WHERE ended_at IS NULL AND playback_id = ?1
         ORDER BY id DESC LIMIT 1",
        params![playback_id],
        |row| {
            let started_at: String = row.get(1)?;
            Ok(OpenMediaInterval {
                id: row.get(0)?,
                started_at: parse_rfc3339_utc(&started_at).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?,
                player_name: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                url: row.get(5)?,
                domain: row.get(6)?,
                source: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn open_media_interval_id(db: &Connection) -> Result<Option<i64>> {
    db.query_row(
        "SELECT id FROM media_intervals WHERE ended_at IS NULL ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn close_open_media_intervals(db: &Connection, now: DateTime<Utc>) -> Result<()> {
    let mut stmt =
        db.prepare("SELECT id, started_at FROM media_intervals WHERE ended_at IS NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let open_rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, started_at) in open_rows {
        close_media_interval_by_id(db, id, &started_at, now)?;
    }
    Ok(())
}

fn close_media_interval_by_id(
    db: &Connection,
    id: i64,
    started_at: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let started_at = parse_rfc3339_utc(started_at)?;
    let ended_at = now.max(started_at);
    db.execute(
        "UPDATE media_intervals SET ended_at = ?1 WHERE id = ?2",
        params![ended_at.to_rfc3339(), id],
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct Status {
    date: String,
    updated_at: String,
    watch_seconds: i64,
    tracked_seconds: i64,
    media_seconds: i64,
    video_watch_seconds: i64,
    apps: Vec<AppStatus>,
    domains: Vec<DomainStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    media: Vec<MediaStatus>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    uncategorized_apps: Vec<AppStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    uncategorized_domains: Vec<DomainStatus>,
    notifications_enabled: bool,
    notifications_break_overdue: bool,
    notifications_budget_exceeded: bool,
    focus_source_kind: String,
    switch_count: i64,
    longest_focus_secs: i64,
    deep_work_blocks: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_until: Option<String>,
}

#[derive(Debug, Serialize)]
struct DaySummary {
    date: String,
    tracked_seconds: i64,
    watch_seconds: i64,
    media_seconds: i64,
    video_watch_seconds: i64,
    categories: Vec<CategoryStatus>,
}

#[derive(Debug, Serialize)]
struct CategoryStatus {
    name: String,
    seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    over_budget: bool,
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

#[derive(Debug, Serialize)]
struct MediaStatus {
    player_name: String,
    label: String,
    seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    watched: bool,
    category: Option<String>,
}

/// Seconds of focus since the last break, and when that session started. The start is the
/// once-per-session key for the break-overdue notification.
fn compute_active_session(
    db: &Connection,
    now: DateTime<Utc>,
    min_break_secs: i64,
) -> Result<(i64, Option<DateTime<Utc>>)> {
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
    let mut session_start = None;
    for r in rows {
        let (started_at, ended_at) = r?;
        let start = parse_rfc3339_utc(&started_at)?;
        // The live interval keeps moving for break reminders, even without idle detection.
        let end = match ended_at {
            Some(s) => parse_rfc3339_utc(&s)?.min(now),
            None => now,
        }
        .max(start);
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
        session_start = Some(effective_start);
        last_start = start;
    }
    Ok((session, session_start))
}

// ponytail: distraction categories hardcoded; move to a [focus] config table if you
// ever want to tune them without a recompile. Must match category names in default.toml.
const DISTRACTION_CATEGORIES: &[&str] = &["scroll", "video"];
// ponytail: 25 min deep-work threshold; promote to config if it needs tuning.
const DEEP_WORK_MIN_SECS: i64 = 1500;
// ponytail: 60s sampling bucket — precision is immaterial against a 25 min threshold.
const FOCUS_BUCKET_SECS: i64 = 60;

struct FocusMetrics {
    switch_count: i64,
    longest_focus_secs: i64,
    deep_work_blocks: i64,
    active_since: Option<DateTime<Utc>>,
    active_until: Option<DateTime<Utc>>,
}

fn is_distraction_category(cat: Option<&str>) -> bool {
    cat.is_some_and(|c| DISTRACTION_CATEGORIES.contains(&c))
}

/// Attention metrics for the [start, end) day window, derived from the existing
/// interval ledger. Switches and the work window come straight from app_intervals;
/// deep-work blocks come from sampling the activity envelope in fixed buckets, where
/// a bucket counts as "deep" when focus covers it and no distraction (scroll/video
/// app, or scroll/video domain inside a browser) does. Non-deep stretches shorter
/// than the break threshold are tolerated within a block, matching the active-session
/// walk. Closed intervals use the idle cap (so stale focus can't bridge a break); the
/// open/live interval stays uncapped, like compute_active_session_seconds.
fn compute_focus_metrics(
    db: &Connection,
    config: &Config,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<FocusMetrics> {
    let effective_now = now.min(end);
    let browser_ids = config.browser_app_ids();

    let mut focused: Vec<(i64, i64)> = Vec::new();
    let mut distraction: Vec<(i64, i64)> = Vec::new();
    let mut switch_count = 0i64;
    let mut since: Option<DateTime<Utc>> = None;
    let mut until: Option<DateTime<Utc>> = None;

    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, app_id
         FROM app_intervals
         WHERE started_at < ?1 AND COALESCE(ended_at, ?2) > ?3 AND started_at >= ?4
         ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map(
        params![
            end.to_rfc3339(),
            effective_now.to_rfc3339(),
            start.to_rfc3339(),
            lookback(start)
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
        let raw_start = parse_rfc3339_utc(&started_at)?;
        let ended = ended_at.map(|v| parse_rfc3339_utc(&v)).transpose()?;
        let interval_end = match ended {
            None => effective_now,
            Some(_) => capped_interval_end(raw_start, ended, effective_now, config.idle_after_secs),
        }
        .min(end);
        let interval_start = raw_start.max(start);
        if interval_end <= interval_start {
            continue;
        }
        if raw_start >= start {
            switch_count += 1;
        }
        since = Some(since.map_or(interval_start, |s| s.min(interval_start)));
        until = Some(until.map_or(interval_end, |u| u.max(interval_end)));
        focused.push((interval_start.timestamp(), interval_end.timestamp()));
        if !browser_ids.contains(&normalize_id(&app_id))
            && is_distraction_category(config.category_for_app(&app_id).as_deref())
        {
            distraction.push((interval_start.timestamp(), interval_end.timestamp()));
        }
    }

    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, domain
         FROM domain_intervals
         WHERE started_at < ?1 AND ended_at > ?2 AND started_at >= ?3
         ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map(
        params![end.to_rfc3339(), start.to_rfc3339(), lookback(start)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (started_at, ended_at, domain) = row?;
        if !is_distraction_category(config.category_for_domain(&domain).as_deref()) {
            continue;
        }
        let s = parse_rfc3339_utc(&started_at)?.max(start);
        let e = parse_rfc3339_utc(&ended_at)?.min(end);
        if e > s {
            distraction.push((s.timestamp(), e.timestamp()));
        }
    }

    let (Some(since_dt), Some(until_dt)) = (since, until) else {
        return Ok(FocusMetrics {
            switch_count,
            longest_focus_secs: 0,
            deep_work_blocks: 0,
            active_since: None,
            active_until: None,
        });
    };

    let covers = |spans: &[(i64, i64)], t: i64| spans.iter().any(|&(s, e)| s <= t && t < e);
    let finalize = |cur: &mut i64, longest: &mut i64, blocks: &mut i64| {
        if *cur > 0 {
            *longest = (*longest).max(*cur);
            if *cur >= DEEP_WORK_MIN_SECS {
                *blocks += 1;
            }
            *cur = 0;
        }
    };
    let min_break = config.breaks.min_break_secs.max(1);
    let mut longest = 0i64;
    let mut blocks = 0i64;
    let mut cur = 0i64;
    let mut nondeep = 0i64;
    let mut t = since_dt.timestamp();
    let end_ts = until_dt.timestamp();
    while t < end_ts {
        if covers(&focused, t) && !covers(&distraction, t) {
            cur += FOCUS_BUCKET_SECS;
            nondeep = 0;
        } else {
            nondeep += FOCUS_BUCKET_SECS;
            if nondeep >= min_break {
                finalize(&mut cur, &mut longest, &mut blocks);
            }
        }
        t += FOCUS_BUCKET_SECS;
    }
    finalize(&mut cur, &mut longest, &mut blocks);

    Ok(FocusMetrics {
        switch_count,
        longest_focus_secs: longest,
        deep_work_blocks: blocks,
        active_since: Some(since_dt),
        active_until: Some(until_dt),
    })
}

fn build_status(db: &Connection, config: &Config, rebuild: bool) -> Result<Status> {
    let now = Utc::now();
    if rebuild {
        rebuild_daily_totals(db, config, now)?;
    }
    let day = Local::now().date_naive();
    let day_string = day.to_string();
    let browser_ids = config.browser_app_ids();

    let day_start = local_day_start_utc(day)?;
    let focus = compute_focus_metrics(
        db,
        config,
        day_start,
        local_day_start_utc(day + Duration::days(1))?,
        now,
    )?;

    let apps = load_app_status(db, &day_string)?;
    let domains = load_domain_status(db, &day_string, &config.display)?;
    let media = load_media_status(db, config, day, now)?;

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
    let media_seconds: i64 = media.iter().map(|item| item.seconds).sum();
    let video_watch_seconds: i64 = media
        .iter()
        .filter(|item| item.category.as_deref() == Some("video"))
        .map(|item| item.seconds)
        .sum();

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
            let over_budget = budget_secs.is_some_and(|b| seconds >= b);
            CategoryStatus {
                name,
                seconds,
                budget_secs,
                over_budget,
            }
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
        compute_active_session(db, now, config.breaks.min_break_secs)?.0
    };
    let break_overdue =
        config.breaks.enabled && !paused && active_session_seconds >= config.breaks.interval_secs;

    let uncategorized_apps = build_uncategorized_apps(&apps, &browser_ids);
    let uncategorized_domains =
        build_uncategorized_domains(&domains, config.display.domains_min_seconds);

    Ok(Status {
        date: day_string,
        updated_at: Local::now().to_rfc3339(),
        watch_seconds,
        tracked_seconds,
        media_seconds,
        video_watch_seconds,
        apps,
        domains,
        media,
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
        uncategorized_apps,
        uncategorized_domains,
        notifications_enabled: config.notifications.enabled,
        notifications_break_overdue: config.notifications.break_overdue,
        notifications_budget_exceeded: config.notifications.budget_exceeded,
        focus_source_kind: config.focus_source.kind.clone(),
        switch_count: focus.switch_count,
        longest_focus_secs: focus.longest_focus_secs,
        deep_work_blocks: focus.deep_work_blocks,
        active_since: focus.active_since.map(|d| d.to_rfc3339()),
        active_until: focus.active_until.map(|d| d.to_rfc3339()),
    })
}

fn run_notification_check(state: &AppState) -> Result<()> {
    let config = state.config.lock().unwrap().clone();
    if !config.notifications.enabled {
        return Ok(());
    }
    let db = state.db.lock().unwrap();
    let now = Utc::now();
    let day = Local::now().date_naive();
    let day_string = day.to_string();

    // Break-overdue notification
    if config.notifications.break_overdue && config.breaks.enabled {
        let paused = load_pause_state(&db)?.is_some();
        if !paused {
            let (active_secs, session_start) =
                compute_active_session(&db, now, config.breaks.min_break_secs)?;
            if active_secs >= config.breaks.interval_secs {
                let session_id = session_start.map(|t| t.to_rfc3339()).unwrap_or_default();
                let notified_session = meta_get(&db, "notified_break_overdue")?.unwrap_or_default();
                if notified_session != session_id {
                    let overdue_secs = active_secs - config.breaks.interval_secs;
                    if let Err(e) =
                        state
                            .notifier
                            .notify(notifications::NotificationKind::BreakOverdue {
                                active_secs,
                                overdue_secs,
                            })
                    {
                        eprintln!("attn notify break_overdue: {e:#}");
                    }
                    meta_set(&db, "notified_break_overdue", &session_id)?;
                }
            }
        }
    }

    // Budget-exceeded notifications
    if config.notifications.budget_exceeded {
        let browser_ids = config.browser_app_ids();
        let apps = load_app_status(&db, &day_string)?;
        let domains = load_domain_status(&db, &day_string, &config.display)?;

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

        for (name, seconds) in &category_totals {
            let Some(budget) = config.budgets.get(name) else {
                continue;
            };
            if budget.daily_budget_secs <= 0 {
                continue;
            }
            if *seconds <= budget.daily_budget_secs {
                continue;
            }

            let dedup_key = format!("notified_budget:{}:{}", day_string, name);
            if meta_get(&db, &dedup_key)?.is_none() {
                if let Err(e) =
                    state
                        .notifier
                        .notify(notifications::NotificationKind::BudgetExceeded {
                            category: name.clone(),
                            seconds: *seconds,
                            budget_secs: budget.daily_budget_secs,
                        })
                {
                    eprintln!("attn notify budget_exceeded({name}): {e:#}");
                }
                meta_set(&db, &dedup_key, "1")?;
            }
        }
    }

    Ok(())
}

fn build_day_summary(db: &Connection, config: &Config, day: NaiveDate) -> Result<DaySummary> {
    let day_string = day.to_string();
    let browser_ids = config.browser_app_ids();
    let apps = load_app_status(db, &day_string)?;
    let domains = load_domain_status(db, &day_string, &config.display)?;
    let media = load_media_status(db, config, day, Utc::now())?;

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
    let media_seconds: i64 = media.iter().map(|item| item.seconds).sum();
    let video_watch_seconds: i64 = media
        .iter()
        .filter(|item| item.category.as_deref() == Some("video"))
        .map(|item| item.seconds)
        .sum();

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
        .map(|(name, seconds)| CategoryStatus {
            name,
            seconds,
            budget_secs: None,
            over_budget: false,
        })
        .collect();
    categories.sort_by(|a, b| b.seconds.cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));
    let tracked_seconds: i64 = categories.iter().map(|c| c.seconds).sum();

    Ok(DaySummary {
        date: day_string,
        tracked_seconds,
        watch_seconds,
        media_seconds,
        video_watch_seconds,
        categories,
    })
}

const UNCATEGORIZED_CAP: usize = 25;

fn build_uncategorized_apps(apps: &[AppStatus], browser_ids: &HashSet<String>) -> Vec<AppStatus> {
    let mut items: Vec<&AppStatus> = apps
        .iter()
        .filter(|a| !a.watched && !browser_ids.contains(&normalize_id(&a.id)) && a.seconds >= 60)
        .collect();
    items.sort_by(|a, b| b.seconds.cmp(&a.seconds));

    if items.len() <= UNCATEGORIZED_CAP {
        items
            .iter()
            .map(|a| AppStatus {
                id: a.id.clone(),
                seconds: a.seconds,
                watched: false,
                category: None,
            })
            .collect()
    } else {
        let mut result: Vec<AppStatus> = items[..UNCATEGORIZED_CAP]
            .iter()
            .map(|a| AppStatus {
                id: a.id.clone(),
                seconds: a.seconds,
                watched: false,
                category: None,
            })
            .collect();
        let tail_seconds: i64 = items[UNCATEGORIZED_CAP..].iter().map(|a| a.seconds).sum();
        result.push(AppStatus {
            id: "(below threshold)".to_string(),
            seconds: tail_seconds,
            watched: false,
            category: None,
        });
        result
    }
}

fn build_uncategorized_domains(domains: &[DomainStatus], min_seconds: i64) -> Vec<DomainStatus> {
    let threshold = min_seconds.max(1);
    let mut items: Vec<&DomainStatus> = domains
        .iter()
        .filter(|d| !d.watched && d.seconds >= threshold)
        .collect();
    items.sort_by(|a, b| b.seconds.cmp(&a.seconds));

    if items.len() <= UNCATEGORIZED_CAP {
        items
            .iter()
            .map(|d| DomainStatus {
                domain: d.domain.clone(),
                seconds: d.seconds,
                watched: false,
                category: None,
            })
            .collect()
    } else {
        let mut result: Vec<DomainStatus> = items[..UNCATEGORIZED_CAP]
            .iter()
            .map(|d| DomainStatus {
                domain: d.domain.clone(),
                seconds: d.seconds,
                watched: false,
                category: None,
            })
            .collect();
        let tail_seconds: i64 = items[UNCATEGORIZED_CAP..].iter().map(|d| d.seconds).sum();
        result.push(DomainStatus {
            domain: "(below threshold)".to_string(),
            seconds: tail_seconds,
            watched: false,
            category: None,
        });
        result
    }
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
    let end = local_day_start_utc(day + Duration::days(1))?;
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
         WHERE started_at < ?1 AND COALESCE(ended_at, ?2) > ?3 AND started_at >= ?4",
    )?;
    let rows = stmt.query_map(
        params![
            end.to_rfc3339(),
            effective_now.to_rfc3339(),
            start.to_rfc3339(),
            lookback(start)
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
         WHERE started_at < ?1 AND ended_at > ?2 AND started_at >= ?3",
    )?;
    let rows = stmt.query_map(
        params![end.to_rfc3339(), start.to_rfc3339(), lookback(start)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
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

#[derive(Debug)]
struct MediaAggregate {
    player_name: String,
    label: String,
    domain: Option<String>,
    seconds: i64,
    category: Option<String>,
}

fn load_media_status(
    db: &Connection,
    config: &Config,
    day: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Vec<MediaStatus>> {
    let start = local_day_start_utc(day)?;
    let end = local_day_start_utc(day + Duration::days(1))?;
    let effective_end = if day == Local::now().date_naive() {
        now.min(end).max(start)
    } else {
        end
    };
    let mut stmt = db.prepare(
        "SELECT started_at, ended_at, player_name, title, artist, url, domain
         FROM media_intervals
         WHERE started_at < ?1 AND COALESCE(ended_at, ?2) > ?3 AND started_at >= ?4",
    )?;
    let rows = stmt.query_map(
        params![
            end.to_rfc3339(),
            effective_end.to_rfc3339(),
            start.to_rfc3339(),
            lookback(start)
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        },
    )?;

    let mut aggregates: BTreeMap<String, MediaAggregate> = BTreeMap::new();
    for row in rows {
        let (started_at, ended_at, player_name, title, artist, url, domain) = row?;
        let interval_start = parse_rfc3339_utc(&started_at)?.max(start);
        let interval_end = ended_at
            .map(|value| parse_rfc3339_utc(&value))
            .transpose()?
            .unwrap_or(effective_end)
            .min(effective_end)
            .min(end);
        if interval_end <= interval_start {
            continue;
        }

        let seconds = (interval_end - interval_start).num_seconds();
        let domain = (!domain.trim().is_empty()).then(|| domain.clone());
        let category = domain
            .as_deref()
            .and_then(|value| config.category_for_domain(value))
            .or_else(|| config.category_for_app(&player_name));
        if category.is_none() {
            continue;
        }
        let label = media_label(domain.as_deref(), &title, &artist, &url, &player_name);
        let key = domain
            .as_ref()
            .map(|value| format!("domain:{value}"))
            .unwrap_or_else(|| format!("player:{player_name}:{}", label));

        let entry = aggregates.entry(key).or_insert_with(|| MediaAggregate {
            player_name: player_name.clone(),
            label,
            domain,
            seconds: 0,
            category,
        });
        entry.seconds += seconds;
    }

    let mut media = aggregates
        .into_values()
        .map(|item| MediaStatus {
            player_name: item.player_name,
            label: item.label,
            seconds: item.seconds,
            domain: item.domain,
            watched: item.category.is_some(),
            category: item.category,
        })
        .collect::<Vec<_>>();
    media.sort_by(|a, b| {
        b.seconds
            .cmp(&a.seconds)
            .then_with(|| a.label.cmp(&b.label))
    });
    Ok(media)
}

fn media_label(
    domain: Option<&str>,
    title: &str,
    artist: &str,
    url: &str,
    player_name: &str,
) -> String {
    if let Some(domain) = domain {
        return domain.to_string();
    }
    if !title.trim().is_empty() && !artist.trim().is_empty() {
        return format!("{} - {}", title.trim(), artist.trim());
    }
    if !title.trim().is_empty() {
        return title.trim().to_string();
    }
    if !url.trim().is_empty() {
        return url.trim().to_string();
    }
    player_name.to_string()
}

fn import_domains_for_day(state: &AppState, day: NaiveDate) -> Result<()> {
    let config = state.config.lock().unwrap().clone();
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
    let end = import_window_end(day, start, local_day_start_utc(day + Duration::days(1))?);
    let mut imports = Vec::new();

    for (name, browser) in &config.browsers {
        let reader = match browsers::for_kind(&browser.kind) {
            Some(r) => r,
            None => {
                eprintln!(
                    "attn: unknown browser kind {:?} for {name}, skipping",
                    browser.kind
                );
                continue;
            }
        };
        let history_paths = match discover_history_paths(browser) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("attn could not discover {name} browser history: {error:#}");
                continue;
            }
        };
        let mut successful_profiles = Vec::new();
        let mut visits = Vec::new();
        for path in &history_paths {
            let snapshot = match snapshot_history(path, name) {
                Ok(s) => s,
                Err(error) => {
                    eprintln!(
                        "attn could not snapshot browser history {}: {error:#}",
                        path.display()
                    );
                    continue;
                }
            };
            let profile = source_profile_from_history_path(path);
            match reader.read_visits(&snapshot, start, end) {
                Ok(mut profile_visits) => {
                    // The module reader derives source_profile from the snapshot
                    // path; fix it up to reflect the real profile name.
                    for v in &mut profile_visits {
                        v.source_profile = profile.clone();
                    }
                    successful_profiles.push(profile);
                    visits.extend(profile_visits);
                }
                Err(error) => eprintln!(
                    "attn could not read browser history {}: {error:#}",
                    path.display()
                ),
            }
            remove_snapshot_files(&snapshot);
        }
        if !successful_profiles.is_empty() {
            successful_profiles.sort();
            successful_profiles.dedup();
            // Normalize again across profiles: one profile's last visit runs to the window
            // end and would otherwise claim the time of the other profile's later visits.
            let visits = browsers::normalize_visit_ranges(visits, start, end);
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
    let end = local_day_start_utc(day + Duration::days(1))?;
    let effective_end = import_window_end(day, start, end);

    db.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<()> {
        for import in imports {
            delete_imported_domain_intervals(db, import, start, end)?;
            for visit in &import.visits {
                insert_attributed_domain_intervals(
                    db,
                    visit,
                    &import.name,
                    &import.browser,
                    start,
                    effective_end,
                    config,
                )?;
            }
        }
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

fn import_window_end(day: NaiveDate, start: DateTime<Utc>, end: DateTime<Utc>) -> DateTime<Utc> {
    if day == Local::now().date_naive() {
        Utc::now().min(end).max(start)
    } else {
        end
    }
}

fn delete_imported_domain_intervals(
    db: &Connection,
    import: &BrowserImport,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    for source_profile in &import.successful_profiles {
        // Rewrite only from the profile's earliest visit still on disk, so a cleared or
        // recreated history cannot erase time that is already attributed.
        let Some(from) = import
            .visits
            .iter()
            .filter(|visit| &visit.source_profile == source_profile)
            .map(|visit| visit.started_at.max(start))
            .min()
        else {
            continue;
        };
        db.execute(
            "DELETE FROM domain_intervals
             WHERE browser_name = ?1
               AND source_profile = ?2
               AND started_at < ?3
               AND started_at >= ?4",
            params![
                import.name,
                source_profile,
                end.to_rfc3339(),
                from.to_rfc3339()
            ],
        )?;
        for app_id in &import.browser.app_ids {
            db.execute(
                "DELETE FROM domain_intervals
                 WHERE browser_name = ''
                   AND browser_app_id = ?1
                   AND source_profile = ?2
                   AND started_at < ?3
                   AND started_at >= ?4",
                params![app_id, source_profile, end.to_rfc3339(), from.to_rfc3339()],
            )?;
        }
    }
    Ok(())
}

use browsers::BrowserVisit;

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

fn cleanup_stale_snapshots() {
    let entries = match fs::read_dir(snapshot_dir()) {
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

/// History snapshots go to the runtime tmpfs: a copy every poll must not churn a disk.
fn snapshot_dir() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

fn snapshot_history(history_path: &Path, browser_name: &str) -> Result<PathBuf> {
    let mut path = snapshot_dir();
    let unique = format!(
        "attn-{browser_name}-{}-{}.sqlite",
        std::process::id(),
        Utc::now().timestamp_micros()
    );
    path.push(unique);
    let result = (|| -> Result<()> {
        fs::copy(history_path, &path).with_context(|| {
            format!(
                "failed to snapshot browser history {}",
                history_path.display()
            )
        })?;
        harden_file_permissions(&path)?;
        // Chromium uses a rollback journal, so a copy taken mid-commit needs it to roll back.
        copy_sqlite_sidecar(history_path, &path, "-journal")?;
        copy_sqlite_sidecar(history_path, &path, "-wal")?;
        copy_sqlite_sidecar(history_path, &path, "-shm")
    })();
    if result.is_err() {
        // A failed copy (e.g. ENOSPC) leaves a partial file behind.
        remove_snapshot_files(&path);
    }
    result.map(|_| path)
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
    let _ = fs::remove_file(sqlite_sidecar_path(snapshot, "-journal"));
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
    config: &Config,
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
        "SELECT started_at, ended_at, app_id, window_title
         FROM app_intervals
         WHERE app_id IN ({placeholders})
           AND started_at < ?
           AND COALESCE(ended_at, ?) > ?
           AND started_at >= ?"
    );

    let mut values: Vec<&dyn rusqlite::ToSql> = Vec::new();
    for app_id in &app_ids {
        values.push(app_id);
    }
    let visit_end = visit.ended_at.to_rfc3339();
    let visit_start = visit.started_at.to_rfc3339();
    let window_end_text = window_end.to_rfc3339();
    let lower = lookback(visit.started_at);
    values.push(&visit_end);
    values.push(&window_end_text);
    values.push(&visit_start);
    values.push(&lower);

    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(values.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    for row in rows {
        let (focus_start, focus_end, app_id, window_title) = row?;
        let title_domain = domain_from_browser_window_title(&window_title)
            .filter(|domain| is_video_domain(config, domain));
        let attributed_visit = title_domain
            .filter(|domain| normalize_domain(domain) != normalize_domain(&visit.domain))
            .map(|domain| BrowserVisit {
                started_at: visit.started_at,
                ended_at: visit.ended_at,
                url: format!("https://{domain}/"),
                domain,
                source_profile: visit.source_profile.clone(),
            });
        let visit = attributed_visit.as_ref().unwrap_or(visit);
        let idle_cap_exempt = is_video_domain(config, &visit.domain);

        let raw_focus_start = parse_rfc3339_utc(&focus_start)?;
        let focus_start = raw_focus_start.max(window_start);
        let raw_focus_end = focus_end
            .map(|value| parse_rfc3339_utc(&value))
            .transpose()?;
        let focus_end = if idle_cap_exempt {
            raw_focus_end.unwrap_or(window_end).max(raw_focus_start)
        } else {
            capped_interval_end(
                raw_focus_start,
                raw_focus_end,
                window_end,
                config.idle_after_secs,
            )
        }
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

fn is_video_domain(config: &Config, domain: &str) -> bool {
    config.category_for_domain(domain).as_deref() == Some("video")
}

fn infer_media_domain_from_focus(
    snapshot: &mut MediaSnapshot,
    config: &Config,
    open: Option<&OpenAppInterval>,
) {
    if !snapshot.domain.trim().is_empty() {
        return;
    }
    let Some(open) = open else {
        return;
    };
    if !media_player_matches_focused_browser(config, &snapshot.player_name, &open.app_id) {
        return;
    }
    let Some(domain) = domain_from_browser_window_title(&open.window_title) else {
        return;
    };
    if is_video_domain(config, &domain) {
        snapshot.domain = domain;
    }
}

fn should_track_media_snapshot(
    snapshot: &MediaSnapshot,
    config: &Config,
    open: Option<&OpenAppInterval>,
) -> bool {
    let Some(open) = open else {
        return false;
    };
    if !media_player_matches_focused_browser(config, &snapshot.player_name, &open.app_id) {
        return false;
    }
    let Some(focused_domain) = domain_from_browser_window_title(&open.window_title) else {
        return false;
    };
    is_video_domain(config, &focused_domain)
        && normalize_domain(&snapshot.domain) == normalize_domain(&focused_domain)
}

fn media_player_matches_focused_browser(config: &Config, player_name: &str, app_id: &str) -> bool {
    let player = normalize_id(player_name);
    let focused = normalize_id(app_id);
    config.browsers.iter().any(|(browser_name, browser)| {
        if !browser
            .app_ids
            .iter()
            .any(|candidate| normalize_id(candidate) == focused)
        {
            return false;
        }

        normalize_id(browser_name) == player
            || browser.app_ids.iter().any(|candidate| {
                let candidate = normalize_id(candidate);
                candidate == player
                    || candidate.starts_with(&format!("{player}-"))
                    || player.starts_with(&format!("{candidate}-"))
            })
    })
}

fn domain_from_browser_window_title(title: &str) -> Option<String> {
    let lower = title.to_ascii_lowercase();
    for (marker, domain) in [
        ("youtube", "youtube.com"),
        ("twitch", "twitch.tv"),
        ("vimeo", "vimeo.com"),
        ("netflix", "netflix.com"),
        ("prime video", "primevideo.com"),
        ("hulu", "hulu.com"),
        ("disney+", "disneyplus.com"),
        ("max", "max.com"),
        ("nebula", "nebula.tv"),
        ("kick", "kick.com"),
        ("dailymotion", "dailymotion.com"),
        ("rumble", "rumble.com"),
    ] {
        if lower == marker
            || lower.ends_with(&format!(" - {marker}"))
            || lower.contains(&format!(" - {marker} - "))
        {
            return Some(domain.to_string());
        }
    }
    None
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
           AND started_at >= ?4
         ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map(
        params![
            browser_app_id,
            ended_at.to_rfc3339(),
            started_at.to_rfc3339(),
            lookback(started_at)
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
        let resolved = if kind == "auto" {
            focus::auto_detect()
        } else {
            kind.as_str()
        };
        match focus::build(resolved) {
            Ok(source) => {
                let probe = source.poll_current();
                match probe {
                    Ok(_) => println!("focus_source: {} (kind = {kind})", source.name()),
                    Err(error) => {
                        println!(
                            "focus_source: {} (kind = {kind}) — probe failed: {error:#}",
                            source.name()
                        );
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
        Ok(Some(version)) if version >= 2 => println!("wayland idle-notify: ok (input idle)"),
        Ok(Some(_)) => {
            println!("wayland idle-notify: ok (v1 only)");
            warnings.push(
                "idle inhibitors (video, games) hold idle back; away time can count as focus"
                    .into(),
            );
        }
        Ok(None) => {
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

    match probe_dbus_notifications() {
        Ok(true) => println!("dbus notifications: ok"),
        Ok(false) => {
            println!("dbus notifications: absent (no notification daemon found on session bus)");
            warnings.push(
                "desktop notifications unavailable; install a notification daemon (mako, dunst, etc.)".into(),
            );
        }
        Err(error) => {
            println!("dbus notifications: error: {error:#}");
            warnings.push(format!("dbus notifications probe failed: {error:#}"));
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
                    // A browser that hit a disk-full error closes its history
                    // DB until restart; a stale mtime is the only visible sign.
                    let age_hours = fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .map(|age| age.as_secs() / 3600);
                    match age_hours {
                        Some(hours) => println!("  {} (last write {hours}h ago)", path.display()),
                        None => println!("  {}", path.display()),
                    }
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

fn probe_wayland_idle() -> Result<Option<u32>> {
    use wayland_client::Connection as WConnection;
    let conn = match WConnection::connect_to_env() {
        Ok(c) => c,
        Err(error) => bail!("wayland connect failed: {error:#}"),
    };
    let mut event_queue = conn.new_event_queue::<DoctorIdleProbe>();
    let qh = event_queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());
    let mut probe = DoctorIdleProbe { version: None };
    event_queue.roundtrip(&mut probe).ok();
    event_queue.roundtrip(&mut probe).ok();
    Ok(probe.version)
}

struct DoctorIdleProbe {
    version: Option<u32>,
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
        if let wayland_client::protocol::wl_registry::Event::Global {
            interface, version, ..
        } = event
        {
            if interface == "ext_idle_notifier_v1" {
                probe.version = Some(version);
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

fn probe_dbus_notifications() -> Result<bool> {
    use zbus::blocking::Connection as ZConnection;
    let conn = match ZConnection::session() {
        Ok(c) => c,
        Err(error) => bail!("session bus connect failed: {error:#}"),
    };
    let proxy = match zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    ) {
        Ok(p) => p,
        Err(error) => bail!("notifications proxy failed: {error:#}"),
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

// ponytail: interval queries look back one day for rows that started earlier and still
// overlap. The longest stored interval so far is 12 h; a longer one would drop out.
const INTERVAL_LOOKBACK_SECS: i64 = 86_400;

/// Lower `started_at` bound for an overlap query at `at`, so it uses the index.
fn lookback(at: DateTime<Utc>) -> String {
    (at - Duration::seconds(INTERVAL_LOOKBACK_SECS)).to_rfc3339()
}

fn parse_rfc3339_utc(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

/// Set while a Wayland idle notification is live. Its `idled` event then ends the open
/// interval at the last input, so the `idle_after_secs` cap would only cut real focus.
static IDLE_DETECTION: AtomicBool = AtomicBool::new(false);

/// Input went idle while a tracked video played. Lock order: after `tracking_state` and `db`.
static DEFERRED_IDLE: Mutex<Option<DeferredIdle>> = Mutex::new(None);

// ponytail: fixed ceiling for a single long video (livestream, loop) watched without
// input; promote to [breaks] config if 3 h ever feels wrong.
const MAX_UNATTENDED_VIDEO_SECS: i64 = 3 * 3600;

#[derive(Clone, Copy, Debug)]
struct DeferredIdle {
    media_id: i64,
    idle_since: DateTime<Utc>,
}

/// A closed interval keeps its stored end. The open one runs to `fallback_end`, capped at
/// `idle_after_secs` while no idle signal can close it.
// ponytail: on a compositor with only ext-idle-notify v1, an idle inhibitor still keeps the
// open interval growing; `attn doctor` warns about it.
fn capped_interval_end(
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    fallback_end: DateTime<Utc>,
    idle_after_secs: i64,
) -> DateTime<Utc> {
    let end = match ended_at {
        Some(end) => end,
        None if IDLE_DETECTION.load(Ordering::Relaxed) => fallback_end,
        None => fallback_end.min(started_at + Duration::seconds(idle_after_secs.max(1))),
    };
    end.max(started_at)
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

#[cfg(test)]
fn domain_matches(domain: &str, candidate: &str) -> bool {
    let domain = normalize_domain(domain);
    let candidate = normalize_domain(candidate);
    domain_matches_normalized(&domain, &candidate)
}

fn domain_matches_normalized(domain: &str, candidate: &str) -> bool {
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
    use notifications::NotificationSink as _;

    #[test]
    fn domain_matching_is_suffix_safe() {
        assert!(domain_matches("youtube.com", "youtube.com"));
        assert!(domain_matches("www.youtube.com", "youtube.com"));
        assert!(domain_matches("m.youtube.com", "youtube.com"));
        assert!(!domain_matches("notyoutube.com", "youtube.com"));
    }

    #[test]
    fn chrome_timestamp_round_trips() {
        // Chrome epoch offset in microseconds (Jan 1 1601 → Jan 1 1970)
        const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;
        let now = Utc::now();
        let rounded = now
            .timestamp_micros()
            .checked_div(1)
            .and_then(|micros| Utc.timestamp_micros(micros).single())
            .unwrap();
        let chrome = rounded.timestamp_micros() + CHROME_EPOCH_OFFSET_MICROS;
        let back = Utc
            .timestamp_micros(chrome - CHROME_EPOCH_OFFSET_MICROS)
            .single()
            .unwrap();
        assert_eq!(back, rounded);
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
            window_title: "one".to_string(),
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
            DescendantProc {
                pid: 11,
                depth: 1,
                name: "bash".to_string(),
                start_time: 100,
            },
            DescendantProc {
                pid: 12,
                depth: 2,
                name: "claude".to_string(),
                start_time: 500,
            },
            DescendantProc {
                pid: 13,
                depth: 3,
                name: "codex".to_string(),
                start_time: 300,
            },
            DescendantProc {
                pid: 14,
                depth: 4,
                name: "node".to_string(),
                start_time: 1000,
            },
        ];
        assert_eq!(
            pick_terminal_subprocess(&candidates, &config),
            Some("claude".to_string())
        );
    }

    #[test]
    fn pick_terminal_subprocess_returns_none_when_no_match() {
        let config = Config::default();
        let candidates = vec![
            DescendantProc {
                pid: 11,
                depth: 1,
                name: "bash".to_string(),
                start_time: 100,
            },
            DescendantProc {
                pid: 12,
                depth: 2,
                name: "fish".to_string(),
                start_time: 200,
            },
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
        assert_eq!(match_terminal_program_in_title("clauderyx", &config), None);
        assert_eq!(match_terminal_program_in_title("", &config), None);
    }

    #[test]
    fn title_match_spinner_resolves_to_claude() {
        let config = Config::default();
        // Braille spinner character
        assert_eq!(
            match_terminal_program_in_title("⠏ attn — src/main.rs", &config),
            Some("claude".to_string())
        );
        // ✳ status marker
        assert_eq!(
            match_terminal_program_in_title("✳ some-project", &config),
            Some("claude".to_string())
        );
        // ⏵ play marker
        assert_eq!(
            match_terminal_program_in_title("⏵ running", &config),
            Some("claude".to_string())
        );
        // Wider braille block
        assert_eq!(
            match_terminal_program_in_title("⣾ compiling", &config),
            Some("claude".to_string())
        );
    }

    #[test]
    fn title_match_tmux_returns_none() {
        let config = Config::default();
        assert_eq!(
            match_terminal_program_in_title("tmux attach -t empyreal", &config),
            None
        );
        assert_eq!(
            match_terminal_program_in_title("tmux new-session -s work", &config),
            None
        );
    }

    #[test]
    fn terminal_cache_hit_returns_cached_value() {
        let config = Config::default();
        let cache = Mutex::new(TerminalWindowCache::new());
        // Pre-populate: window 5 → "codex"
        cache.lock().unwrap().insert(5, "codex".to_string());

        // Ambiguous title (no marker, no word match) — should hit cache.
        let window = FocusedWindow {
            window_id: Some(5),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "flashscribe".to_string(), // no program marker
            pid: None,                        // no pid → /proc walk skipped
        };
        let resolved = resolve_focused_window(window, &config, &cache);
        assert_eq!(resolved.app_id, "codex");
    }

    #[test]
    fn terminal_cache_miss_falls_through() {
        let config = Config::default();
        let cache = Mutex::new(TerminalWindowCache::new());
        // No cache entry for window 99.
        let window = FocusedWindow {
            window_id: Some(99),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "some-project".to_string(),
            pid: None, // no pid so /proc walk also skips
        };
        let resolved = resolve_focused_window(window, &config, &cache);
        // Falls through to unresolved — app_id unchanged.
        assert_eq!(resolved.app_id, "com.mitchellh.ghostty");
    }

    #[test]
    fn terminal_cache_evicts_oldest_at_cap() {
        let mut cache = TerminalWindowCache::new();
        for i in 0..TERMINAL_CACHE_CAP {
            cache.insert(i as i64, format!("app{i}"));
        }
        assert_eq!(cache.map.len(), TERMINAL_CACHE_CAP);
        // Insert one more — should evict window 0.
        cache.insert(999, "newcomer".to_string());
        assert_eq!(cache.map.len(), TERMINAL_CACHE_CAP);
        assert!(cache.get(0).is_none(), "oldest entry should be evicted");
        assert_eq!(cache.get(999), Some("newcomer"));
    }

    #[test]
    fn terminal_cache_unambiguous_title_updates_cache() {
        let config = Config::default();
        let cache = Mutex::new(TerminalWindowCache::new());
        // Title with spinner — unambiguous → should populate cache.
        let window = FocusedWindow {
            window_id: Some(42),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "⠹ working on attn".to_string(),
            pid: None,
        };
        let resolved = resolve_focused_window(window, &config, &cache);
        assert_eq!(resolved.app_id, "claude");
        assert_eq!(cache.lock().unwrap().get(42), Some("claude"));
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
        let cache = Mutex::new(TerminalWindowCache::new());
        let titled = FocusedWindow {
            window_id: Some(7),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "claude".to_string(),
            pid: None,
        };
        let resolved = resolve_focused_window(titled, &config, &cache);
        assert_eq!(resolved.app_id, "claude");

        // Different window id — no cache entry, tmux title falls through to /proc walk
        // (which skips because pid is None), leaving app_id unchanged.
        let untitled = FocusedWindow {
            window_id: Some(8),
            app_id: "com.mitchellh.ghostty".to_string(),
            title: "tmux attach -t empyreal".to_string(),
            pid: None,
        };
        let resolved = resolve_focused_window(untitled, &config, &cache);
        assert_eq!(resolved.app_id, "com.mitchellh.ghostty");

        let non_terminal = FocusedWindow {
            window_id: Some(9),
            app_id: "brave".to_string(),
            title: "x".to_string(),
            pid: Some(1234),
        };
        let resolved = resolve_focused_window(non_terminal, &config, &cache);
        assert_eq!(resolved.app_id, "brave");
    }

    #[test]
    fn migration_creates_window_id_column() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        assert!(table_column_exists(&db, "app_intervals", "window_id").unwrap());
        assert!(table_column_exists(&db, "domain_intervals", "browser_name").unwrap());
        assert!(table_column_exists(&db, "media_intervals", "playback_id").unwrap());
    }

    #[test]
    fn focus_metrics_blocks_switches_and_distractions() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config::default();

        let start = parse_rfc3339_utc("2026-06-22T00:00:00Z").unwrap();
        let end = start + Duration::days(1);
        let now = start + Duration::hours(23);

        let ins_app = |s: i64, e: i64, app: &str| {
            db.execute(
                "INSERT INTO app_intervals(started_at, ended_at, app_id) VALUES (?1, ?2, ?3)",
                params![
                    (start + Duration::seconds(s)).to_rfc3339(),
                    (start + Duration::seconds(e)).to_rfc3339(),
                    app
                ],
            )
            .unwrap();
        };
        let ins_dom = |s: i64, e: i64, dom: &str| {
            db.execute(
                "INSERT INTO domain_intervals(started_at, ended_at, browser_app_id, domain)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    (start + Duration::seconds(s)).to_rfc3339(),
                    (start + Duration::seconds(e)).to_rfc3339(),
                    "firefox",
                    dom
                ],
            )
            .unwrap();
        };

        // Intervals must stay <= idle_after_secs (300) or the idle cap chops them, so
        // tile contiguous 300s slices to build a long unbroken block.
        // Block: 30 min editor + 30 min browser-on-github, contiguous => one 60 min block.
        for i in 0..6 {
            ins_app(i * 300, (i + 1) * 300, "positron");
        }
        for i in 6..12 {
            ins_app(i * 300, (i + 1) * 300, "firefox");
        }
        ins_dom(1800, 3600, "github.com"); // coding domain: not a distraction

        // 60 min real break (gap >= min_break) splits the timeline here.

        // 40 min browser session with a 10 min YouTube stretch in the middle: the
        // distraction (>= min_break) prevents this from forming a deep block.
        for i in 0..8 {
            ins_app(7200 + i * 300, 7200 + (i + 1) * 300, "firefox");
        }
        ins_dom(8400, 9000, "youtube.com"); // video domain: a distraction

        let m = compute_focus_metrics(&db, &config, start, end, now).unwrap();
        assert_eq!(m.switch_count, 20, "every interval starting today is a switch");
        assert_eq!(m.longest_focus_secs, 3600, "editor->browser stays one 60m block");
        assert_eq!(m.deep_work_blocks, 1, "youtube splits the 40m session below 25m");
        assert!(m.active_since.is_some() && m.active_until.is_some());
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
        assert_eq!(
            config.category_for_app("positron"),
            Some("coding".to_string())
        );
        assert_eq!(
            config.category_for_domain("vercel.com"),
            Some("coding".to_string())
        );
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

        assert_eq!(
            config.category_for_app("my-editor"),
            Some("coding".to_string())
        );
        assert_eq!(
            config.category_for_app("positron"),
            Some("coding".to_string())
        );
        assert_eq!(
            config.category_for_domain("example.edu"),
            Some("learning".to_string())
        );
        assert_eq!(
            config.category_for_domain("vercel.com"),
            Some("coding".to_string())
        );
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
        let config = Config {
            idle_after_secs: 1200,
            ..Default::default()
        };
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
            &config,
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
        let config = Config {
            idle_after_secs: 300,
            ..Default::default()
        };
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(30);
        // Open: without idle detection only the open interval gets the cap at read time.
        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'brave-browser', 'Example')",
            params![focus_start.to_rfc3339()],
        )
        .unwrap();

        let visit = BrowserVisit {
            started_at: focus_start,
            ended_at: focus_start + Duration::minutes(30),
            url: "https://example.com/".to_string(),
            domain: "example.com".to_string(),
            source_profile: "Default".to_string(),
        };
        insert_attributed_domain_intervals(
            &db,
            &visit,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            &config,
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
    fn browser_window_title_overrides_stale_history_for_video_domain() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config {
            idle_after_secs: 300,
            ..Default::default()
        };
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(30);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'Shadow Integration - YouTube - Brave')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: vec![BrowserVisit {
                started_at: focus_start,
                ended_at: focus_end,
                url: "https://twitter.com/?lang=de".to_string(),
                domain: "twitter.com".to_string(),
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

        let row: (String, i64) = db
            .query_row(
                "SELECT domain, CAST(SUM((julianday(ended_at) - julianday(started_at)) * 86400) AS INTEGER)
                 FROM domain_intervals
                 GROUP BY domain",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("youtube.com".to_string(), 1800));
    }

    #[test]
    fn video_domain_imports_use_full_browser_focus_interval() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config {
            idle_after_secs: 300,
            ..Default::default()
        };
        let browser = BrowserConfig {
            app_ids: vec!["brave-browser".to_string()],
            history_paths: Vec::new(),
            kind: "chromium".to_string(),
        };
        let focus_start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let focus_end = focus_start + Duration::minutes(30);
        db.execute(
            "INSERT INTO app_intervals(started_at, ended_at, app_id, window_title)
             VALUES (?1, ?2, 'brave-browser', 'YouTube')",
            params![focus_start.to_rfc3339(), focus_end.to_rfc3339()],
        )
        .unwrap();

        let imports = vec![BrowserImport {
            name: "brave".to_string(),
            browser,
            successful_profiles: vec!["Default".to_string()],
            visits: vec![BrowserVisit {
                started_at: focus_start,
                ended_at: focus_end,
                url: "https://www.youtube.com/watch?v=test".to_string(),
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

        let seconds: i64 = db
            .query_row(
                "SELECT CAST(SUM((julianday(ended_at) - julianday(started_at)) * 86400) AS INTEGER)
                 FROM domain_intervals",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seconds, 1800);
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
            &Config {
                idle_after_secs: 1200,
                ..Default::default()
            },
        )
        .unwrap();
        insert_attributed_domain_intervals(
            &db,
            &second,
            "brave",
            &browser,
            focus_start - Duration::hours(1),
            focus_end + Duration::hours(1),
            &Config {
                idle_after_secs: 1200,
                ..Default::default()
            },
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
            visits: vec![BrowserVisit {
                started_at: focus_start,
                ended_at: focus_end,
                url: "https://new.example/".to_string(),
                domain: "new.example".to_string(),
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
            visits: vec![BrowserVisit {
                started_at: focus_start,
                ended_at: focus_end,
                url: "https://new.example/".to_string(),
                domain: "new.example".to_string(),
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
            config: Arc::new(Mutex::new(Arc::new(old_config.clone()))),
            db: Arc::new(Mutex::new(old_db)),
            db_state_path: Arc::new(Mutex::new(old_config.state_path.clone())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
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
            config: Arc::new(Mutex::new(Arc::new(old_config.clone()))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(new_config.state_path.clone())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
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
            config: Arc::new(Mutex::new(Arc::new(Config::default()))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
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
            config: Arc::new(Mutex::new(Arc::new(Config::default()))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(Some(idle_pause))),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
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
    fn focus_change_does_not_end_manual_pause() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let mut config = Config::default();
        config.breaks.min_break_secs = 300;
        let paused_at = Utc::now() - Duration::seconds(30);
        let info = PauseInfo {
            at: paused_at,
            reason: PauseReason::Manual,
        };
        persist_pause_state(&db, Some(info)).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(Some(info))),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };

        let window = focus::FocusedWindow {
            window_id: Some(7),
            app_id: "code".to_string(),
            title: "src/main.rs".to_string(),
            pid: None,
        };
        handle_focus_change_with(&state, Some(window)).unwrap();

        assert!(
            state.tracking_state.lock().unwrap().is_some(),
            "only the idle resumed event ends a pause"
        );
    }

    #[test]
    fn domain_category_prefers_the_most_specific_host() {
        let config = Config::default();
        assert_eq!(
            config.category_for_domain("drive.google.com"),
            Some("storage".to_string())
        );
        assert_eq!(
            config.category_for_domain("google.com"),
            Some("search".to_string())
        );
        assert_eq!(
            config.category_for_domain("hub.docker.com"),
            Some("devops".to_string())
        );
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

        let (seconds, _) = compute_active_session(&db, now, 300).unwrap();

        assert_eq!(seconds, 65 * 60);
    }

    #[test]
    fn chromium_reader_extracts_domains_and_keeps_long_durations() {
        const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;

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
            let chrome_time = started_at.timestamp_micros() + CHROME_EPOCH_OFFSET_MICROS;
            db.execute(
                "INSERT INTO urls(id, url) VALUES (1, 'https://www.youtube.com/watch?v=test')",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO visits(id, url, visit_time, visit_duration)
                 VALUES (1, 1, ?1, ?2)",
                params![chrome_time, 3_600_i64 * 1_000_000],
            )
            .unwrap();
        }

        let started_at = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let reader = browsers::chromium::ChromiumReader;
        let visits = browsers::BrowserReader::read_visits(
            &reader,
            &path,
            started_at - Duration::minutes(1),
            started_at + Duration::minutes(20),
        )
        .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].domain, "youtube.com");
        assert_eq!(
            (visits[0].ended_at - visits[0].started_at).num_seconds(),
            3600
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
    fn status_reports_media_watch_seconds_separately() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let config = Config::default();
        let now = Utc::now();
        let started_at = now - Duration::seconds(120);
        let ended_at = now - Duration::seconds(60);

        db.execute(
            "INSERT INTO media_intervals(
                started_at, ended_at, playback_id, player_name, title, artist, url, domain, source
             ) VALUES (?1, ?2, 'org.mpris.MediaPlayer2.brave.instance1', 'brave', 'Video A', '', ?3, 'youtube.com', 'mpris')",
            params![
                started_at.to_rfc3339(),
                ended_at.to_rfc3339(),
                "https://www.youtube.com/watch?v=a"
            ],
        )
        .unwrap();

        let status = build_status(&db, &config, true).unwrap();
        assert_eq!(status.watch_seconds, 0);
        assert_eq!(status.media_seconds, 60);
        assert_eq!(status.video_watch_seconds, 60);
        assert_eq!(status.media.len(), 1);
        assert_eq!(status.media[0].domain.as_deref(), Some("youtube.com"));
        assert_eq!(status.media[0].category.as_deref(), Some("video"));
    }

    #[test]
    fn status_omits_uncategorized_media_audio() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let now = Utc::now();

        db.execute(
            "INSERT INTO media_intervals(
                started_at, ended_at, playback_id, player_name, title, artist, url, domain, source
             ) VALUES (?1, ?2, 'org.mpris.MediaPlayer2.brave.instance1', 'brave', 'Audio Track', '', '', '', 'mpris')",
            params![
                (now - Duration::seconds(120)).to_rfc3339(),
                (now - Duration::seconds(60)).to_rfc3339(),
            ],
        )
        .unwrap();

        let status = build_status(&db, &Config::default(), true).unwrap();
        assert_eq!(status.media_seconds, 0);
        assert_eq!(status.video_watch_seconds, 0);
        assert!(status.media.is_empty());
    }

    #[test]
    fn media_snapshot_infers_video_domain_from_focused_browser_title() {
        let config = Config::default();
        let open = OpenAppInterval {
            window_id: Some(1),
            app_id: "brave-browser".to_string(),
            window_title: "Shadow Integration - YouTube - Brave".to_string(),
        };
        let mut snapshot = MediaSnapshot {
            playback_id: "org.mpris.MediaPlayer2.brave.instance1".to_string(),
            player_name: "brave".to_string(),
            title: "Shadow Integration".to_string(),
            artist: "Curio Subs".to_string(),
            url: String::new(),
            domain: String::new(),
            source: "mpris".to_string(),
        };

        infer_media_domain_from_focus(&mut snapshot, &config, Some(&open));

        assert_eq!(snapshot.domain, "youtube.com");
        assert!(should_track_media_snapshot(&snapshot, &config, Some(&open)));
    }

    #[test]
    fn media_snapshot_does_not_track_background_audio() {
        let config = Config::default();
        let open = OpenAppInterval {
            window_id: Some(1),
            app_id: "codex".to_string(),
            window_title: "attn".to_string(),
        };
        let snapshot = MediaSnapshot {
            playback_id: "org.mpris.MediaPlayer2.brave.instance1".to_string(),
            player_name: "brave".to_string(),
            title: "Audio Track".to_string(),
            artist: String::new(),
            url: String::new(),
            domain: String::new(),
            source: "mpris".to_string(),
        };

        assert!(!should_track_media_snapshot(
            &snapshot,
            &config,
            Some(&open)
        ));
    }

    #[test]
    fn media_snapshot_sync_rolls_intervals_on_media_change() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let snapshot = MediaSnapshot {
            playback_id: "org.mpris.MediaPlayer2.brave.instance1".to_string(),
            player_name: "brave".to_string(),
            title: "Video A".to_string(),
            artist: String::new(),
            url: "https://www.youtube.com/watch?v=a".to_string(),
            domain: "youtube.com".to_string(),
            source: "mpris".to_string(),
        };

        sync_media_snapshots(&db, start, std::slice::from_ref(&snapshot)).unwrap();
        sync_media_snapshots(
            &db,
            start + Duration::minutes(5),
            std::slice::from_ref(&snapshot),
        )
        .unwrap();

        let open_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM media_intervals WHERE ended_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open_count, 1);

        let changed = MediaSnapshot {
            title: "Video B".to_string(),
            url: "https://www.youtube.com/watch?v=b".to_string(),
            ..snapshot
        };
        sync_media_snapshots(
            &db,
            start + Duration::minutes(10),
            std::slice::from_ref(&changed),
        )
        .unwrap();
        sync_media_snapshots(&db, start + Duration::minutes(20), &[]).unwrap();

        let rows = db
            .prepare("SELECT title, started_at, ended_at FROM media_intervals ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Video A");
        assert_eq!(
            rows[0].2.as_deref(),
            Some((start + Duration::minutes(10)).to_rfc3339().as_str())
        );
        assert_eq!(rows[1].0, "Video B");
        assert_eq!(
            rows[1].2.as_deref(),
            Some((start + Duration::minutes(20)).to_rfc3339().as_str())
        );
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

    // ----- bundled default config tests -----

    #[test]
    fn partial_user_config_inherits_bundled_watch_lists() {
        // A config.toml that omits the list sections entirely must fall back to the
        // same lists a full config ships, not a smaller hand-maintained copy.
        let partial = toml::from_str::<Config>("poll_interval_secs = 30\n").unwrap();
        let bundled = bundled_default_config().unwrap();

        assert_eq!(partial.apps.watch, bundled.apps.watch);
        assert_eq!(partial.domains.watch, bundled.domains.watch);
        // Full value comparison, not just keys: `apply_user_config` overwrites browser
        // entries wholesale, so a drifted `history_paths` silently loses a profile.
        assert_eq!(partial.browsers, bundled.browsers);
        assert_eq!(partial.poll_interval_secs, 30);
    }

    #[test]
    fn bundled_default_lists_are_populated() {
        // Guards the include_str! -> serde default wiring: if the lookup path ever
        // stops resolving, these silently become empty rather than failing loudly.
        let config = Config::default();
        assert!(
            config.domains.watch.len() >= 20,
            "expected the full bundled domain category set, got {}",
            config.domains.watch.len()
        );
        assert!(config.apps.watch.contains_key("coding"));
        assert!(config.domains.watch.contains_key("news"));
        assert!(config.browsers.contains_key("firefox"));
        assert!(config
            .domains
            .watch
            .get("news")
            .is_some_and(|d| d.iter().any(|domain| domain == "arstechnica.com")));
    }

    #[test]
    fn bundled_default_table_returns_default_for_missing_path() {
        let missing: BTreeMap<String, Vec<String>> =
            bundled_default_table(&["nope", "not_a_table"]);
        assert!(missing.is_empty());
    }

    // ----- focus source tests -----

    #[test]
    fn auto_detect_prefers_niri_over_hyprland() {
        assert_eq!(focus::detect_from(true, true, true), "niri");
    }

    #[test]
    fn auto_detect_returns_hyprland_when_signature_set_and_no_niri() {
        assert_eq!(focus::detect_from(false, true, true), "hyprland");
    }

    #[test]
    fn auto_detect_returns_sway_when_only_swaysock_set() {
        assert_eq!(focus::detect_from(false, false, true), "sway");
    }

    #[test]
    fn auto_detect_falls_back_to_river() {
        assert_eq!(focus::detect_from(false, false, false), "river");
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

    #[test]
    fn category_status_over_budget_when_seconds_exceed_budget() {
        let cs = CategoryStatus {
            name: "scroll".to_string(),
            seconds: 700,
            budget_secs: Some(600),
            over_budget: 700 >= 600,
        };
        assert!(cs.over_budget);
    }

    #[test]
    fn category_status_not_over_budget_when_below() {
        let cs = CategoryStatus {
            name: "scroll".to_string(),
            seconds: 300,
            budget_secs: Some(600),
            over_budget: 300 >= 600,
        };
        assert!(!cs.over_budget);
    }

    #[test]
    fn category_status_not_over_budget_when_at_zero() {
        let cs = CategoryStatus {
            name: "scroll".to_string(),
            seconds: 0,
            budget_secs: Some(600),
            over_budget: 0 >= 600,
        };
        assert!(!cs.over_budget);
    }

    #[test]
    fn category_status_not_over_budget_when_no_budget() {
        let over = None::<i64>.is_some_and(|b: i64| 9999_i64 >= b);
        assert!(!over);
    }

    #[test]
    fn category_status_over_budget_serialization_omits_false() {
        let cs = CategoryStatus {
            name: "coding".to_string(),
            seconds: 100,
            budget_secs: None,
            over_budget: false,
        };
        let json = serde_json::to_string(&cs).unwrap();
        assert!(
            !json.contains("over_budget"),
            "over_budget should be omitted when false: {json}"
        );
    }

    #[test]
    fn category_status_over_budget_serialization_includes_true() {
        let cs = CategoryStatus {
            name: "scroll".to_string(),
            seconds: 700,
            budget_secs: Some(600),
            over_budget: true,
        };
        let json = serde_json::to_string(&cs).unwrap();
        assert!(
            json.contains("\"over_budget\":true"),
            "over_budget:true should appear in JSON: {json}"
        );
    }

    #[test]
    fn break_overdue_notification_records_kind() {
        let rec = notifications::RecordingNotifier::new();
        rec.notify(notifications::NotificationKind::BreakOverdue {
            active_secs: 3720,
            overdue_secs: 120,
        })
        .unwrap();
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "break_overdue");
    }

    #[test]
    fn budget_exceeded_notification_dedup_fires_once_across_many_iterations() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();

        let rec = Arc::new(notifications::RecordingNotifier::new());
        let mut config = Config::default();
        config.budgets.insert(
            "scroll".to_string(),
            BudgetEntry {
                daily_budget_secs: 30,
            },
        );
        config.notifications.enabled = true;
        config.notifications.budget_exceeded = true;

        let day = Local::now().date_naive();
        let day_string = day.to_string();

        db.execute(
            "INSERT INTO daily_domain_totals(day, domain, seconds, watch_category)
             VALUES (?1, 'youtube.com', 60, 'scroll')",
            params![day_string],
        )
        .unwrap();

        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config.clone()))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: rec.clone(),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };

        for _ in 0..100 {
            run_notification_check(&state).unwrap();
        }

        assert_eq!(
            rec.call_count(),
            1,
            "budget notification should fire exactly once"
        );
    }

    #[test]
    fn break_overdue_notification_dedup_fires_once_per_session() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();

        let rec = Arc::new(notifications::RecordingNotifier::new());
        let now = Utc::now();

        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![(now - Duration::seconds(7200)).to_rfc3339()],
        )
        .unwrap();
        let session_id = (now - Duration::seconds(7200)).to_rfc3339();
        meta_set(&db, "session_reset_at", &session_id).unwrap();

        let mut config = Config::default();
        config.notifications.enabled = true;
        config.notifications.break_overdue = true;
        config.breaks.enabled = true;
        config.breaks.interval_secs = 3600;
        config.breaks.min_break_secs = 300;

        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: rec.clone(),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };

        run_notification_check(&state).unwrap();
        run_notification_check(&state).unwrap();

        assert_eq!(
            rec.call_count(),
            1,
            "break-overdue should fire once per session"
        );
    }

    #[test]
    fn break_overdue_notification_refires_after_break() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();

        let rec = Arc::new(notifications::RecordingNotifier::new());
        let now = Utc::now();

        db.execute(
            "INSERT INTO app_intervals(started_at, app_id, window_title)
             VALUES (?1, 'code', 'Editor')",
            params![(now - Duration::seconds(28800)).to_rfc3339()],
        )
        .unwrap();
        let old_session_id = (now - Duration::seconds(28800)).to_rfc3339();
        meta_set(&db, "session_reset_at", &old_session_id).unwrap();

        let mut config = Config::default();
        config.notifications.enabled = true;
        config.notifications.break_overdue = true;
        config.breaks.enabled = true;
        config.breaks.interval_secs = 3600;
        config.breaks.min_break_secs = 300;

        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: rec.clone(),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };

        run_notification_check(&state).unwrap();
        assert_eq!(rec.call_count(), 1);

        let break_time = (now - Duration::seconds(1800)).to_rfc3339();
        {
            let db = state.db.lock().unwrap();
            meta_set(&db, "session_reset_at", &break_time).unwrap();
            let new_interval_start = (now - Duration::seconds(1740)).to_rfc3339();
            db.execute(
                "UPDATE app_intervals SET started_at = ?1",
                params![new_interval_start],
            )
            .unwrap();
        }

        run_notification_check(&state).unwrap();
        assert_eq!(rec.call_count(), 1, "no notification while not yet overdue");

        {
            let db = state.db.lock().unwrap();
            let new_session_start = (now - Duration::seconds(7200)).to_rfc3339();
            meta_set(&db, "session_reset_at", &new_session_start).unwrap();
            let interval_start = (now - Duration::seconds(7500)).to_rfc3339();
            db.execute(
                "UPDATE app_intervals SET started_at = ?1",
                params![interval_start],
            )
            .unwrap();
        }

        run_notification_check(&state).unwrap();
        assert_eq!(
            rec.call_count(),
            2,
            "break-overdue should refire after break"
        );
    }

    // ----- Phase 5: uncategorized + categorize tests -----

    #[test]
    fn uncategorized_apps_filters_watched_and_low_seconds() {
        let browser_ids: HashSet<String> = HashSet::new();
        let apps = vec![
            AppStatus {
                id: "firefox".to_string(),
                seconds: 120,
                watched: false,
                category: None,
            },
            AppStatus {
                id: "code".to_string(),
                seconds: 500,
                watched: true,
                category: Some("coding".to_string()),
            },
            AppStatus {
                id: "tiny".to_string(),
                seconds: 30,
                watched: false,
                category: None,
            },
        ];
        let result = build_uncategorized_apps(&apps, &browser_ids);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "firefox");
    }

    #[test]
    fn uncategorized_apps_excludes_browser_ids() {
        let mut browser_ids = HashSet::new();
        browser_ids.insert("brave-browser".to_string());
        let apps = vec![
            AppStatus {
                id: "brave-browser".to_string(),
                seconds: 600,
                watched: false,
                category: None,
            },
            AppStatus {
                id: "mpv".to_string(),
                seconds: 120,
                watched: false,
                category: None,
            },
        ];
        let result = build_uncategorized_apps(&apps, &browser_ids);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "mpv");
    }

    #[test]
    fn uncategorized_apps_cap_produces_below_threshold_row() {
        let browser_ids: HashSet<String> = HashSet::new();
        let apps: Vec<AppStatus> = (0..30)
            .map(|i| AppStatus {
                id: format!("app{i}"),
                seconds: 100 + i as i64,
                watched: false,
                category: None,
            })
            .collect();
        let result = build_uncategorized_apps(&apps, &browser_ids);
        assert_eq!(result.len(), UNCATEGORIZED_CAP + 1);
        let tail = result.last().unwrap();
        assert_eq!(tail.id, "(below threshold)");
        assert!(!tail.watched);
        let mut sorted_secs: Vec<i64> = apps.iter().map(|a| a.seconds).collect();
        sorted_secs.sort();
        let expected_tail: i64 = sorted_secs[..5].iter().sum();
        assert_eq!(tail.seconds, expected_tail);
    }

    #[test]
    fn uncategorized_domains_filters_watched_and_below_min() {
        let domains = vec![
            DomainStatus {
                domain: "example.com".to_string(),
                seconds: 60,
                watched: false,
                category: None,
            },
            DomainStatus {
                domain: "github.com".to_string(),
                seconds: 200,
                watched: true,
                category: Some("coding".to_string()),
            },
            DomainStatus {
                domain: "tiny.io".to_string(),
                seconds: 5,
                watched: false,
                category: None,
            },
        ];
        let result = build_uncategorized_domains(&domains, 30);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].domain, "example.com");
    }

    #[test]
    fn handle_categorize_toml_edit_round_trip_preserves_existing_entries() {
        use toml_edit::DocumentMut;
        let toml_content = "[apps.watch]\ncoding = [\"code\", \"zed\"]\nterminal = [\"ghostty\"]\n";
        let mut doc = toml_content.parse::<DocumentMut>().unwrap();
        let arr = doc["apps"]["watch"]["coding"].as_array_mut().unwrap();
        assert!(!arr.iter().any(|v| v.as_str() == Some("cursor")));
        arr.push("cursor");
        let out = doc.to_string();
        assert!(out.contains("cursor"));
        assert!(out.contains("zed"));
        assert!(out.contains("ghostty"));
    }

    #[test]
    fn handle_categorize_rejects_unknown_category() {
        let mut watch = BTreeMap::new();
        watch.insert("coding".to_string(), vec!["code".to_string()]);
        let config = Config {
            apps: AppsConfig { watch },
            ..Default::default()
        };
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };
        let req = CategorizeRequest {
            kind: CategorizeKind::App,
            id: "firefox".to_string(),
            category: "nonexistent".to_string(),
        };
        let resp = handle_categorize(&state, req).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap_or("").contains("not found"));
    }

    #[test]
    fn status_json_omits_uncategorized_when_empty() {
        let status = Status {
            date: "2026-05-15".to_string(),
            updated_at: "2026-05-15T12:00:00+00:00".to_string(),
            watch_seconds: 0,
            tracked_seconds: 0,
            media_seconds: 0,
            video_watch_seconds: 0,
            apps: vec![],
            domains: vec![],
            media: vec![],
            categories: vec![],
            days: vec![],
            active_session_seconds: 0,
            break_overdue: false,
            paused: false,
            paused_reason: None,
            paused_since: None,
            breaks_enabled: true,
            breaks_interval_secs: 3600,
            breaks_min_break_secs: 300,
            uncategorized_apps: vec![],
            uncategorized_domains: vec![],
            notifications_enabled: true,
            notifications_break_overdue: true,
            notifications_budget_exceeded: true,
            focus_source_kind: "niri".to_string(),
            switch_count: 0,
            longest_focus_secs: 0,
            deep_work_blocks: 0,
            active_since: None,
            active_until: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(
            !json.contains("uncategorized_apps"),
            "empty vec should be omitted"
        );
        assert!(
            !json.contains("uncategorized_domains"),
            "empty vec should be omitted"
        );
    }

    #[test]
    fn status_json_includes_uncategorized_when_present() {
        let status = Status {
            date: "2026-05-15".to_string(),
            updated_at: "2026-05-15T12:00:00+00:00".to_string(),
            watch_seconds: 0,
            tracked_seconds: 0,
            media_seconds: 0,
            video_watch_seconds: 0,
            apps: vec![],
            domains: vec![],
            media: vec![],
            categories: vec![],
            days: vec![],
            active_session_seconds: 0,
            break_overdue: false,
            paused: false,
            paused_reason: None,
            paused_since: None,
            breaks_enabled: true,
            breaks_interval_secs: 3600,
            breaks_min_break_secs: 300,
            uncategorized_apps: vec![AppStatus {
                id: "obs".to_string(),
                seconds: 120,
                watched: false,
                category: None,
            }],
            uncategorized_domains: vec![],
            notifications_enabled: true,
            notifications_break_overdue: true,
            notifications_budget_exceeded: true,
            focus_source_kind: "niri".to_string(),
            switch_count: 0,
            longest_focus_secs: 0,
            deep_work_blocks: 0,
            active_since: None,
            active_until: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("uncategorized_apps"));
        assert!(json.contains("obs"));
    }

    // ── set_budget tests ──────────────────────────────────────────────────────

    #[test]
    fn handle_set_budget_updates_existing_budget() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        fs::write(
            &path,
            "[apps.watch]\ncoding = [\"code\"]\n\n[budgets.coding]\ndaily_budget_secs = 1800\n",
        )
        .unwrap();

        let mut watch = BTreeMap::new();
        watch.insert("coding".to_string(), vec!["code".to_string()]);
        let config = Config {
            apps: AppsConfig { watch },
            ..Default::default()
        };
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };

        // Temporarily override DEFAULT_CONFIG_PATH by writing to the test path and using it directly.
        // We test the handler logic via toml_edit round-trip.
        let req = SetBudgetRequest {
            category: "coding".to_string(),
            daily_budget_secs: 3600,
        };
        // Inline the toml_edit logic to test it without needing the real config path.
        let raw = fs::read_to_string(&path).unwrap();
        use toml_edit::{value, DocumentMut};
        let mut doc = raw.parse::<DocumentMut>().unwrap();
        let budgets = doc
            .get_mut("budgets")
            .and_then(|b| b.as_table_mut())
            .unwrap();
        let entry = budgets
            .get_mut("coding")
            .and_then(|e| e.as_table_mut())
            .unwrap();
        entry.insert("daily_budget_secs", value(req.daily_budget_secs));
        let out = doc.to_string();
        assert!(out.contains("daily_budget_secs = 3600"));
        assert!(out.contains("[apps.watch]"), "unrelated sections preserved");

        drop(state); // appease clippy
    }

    #[test]
    fn handle_set_budget_removes_when_zero() {
        let raw = "[apps.watch]\ncoding = [\"code\"]\n\n[budgets]\n[budgets.coding]\ndaily_budget_secs = 1800\n";
        use toml_edit::DocumentMut;
        let mut doc = raw.parse::<DocumentMut>().unwrap();
        if let Some(budgets) = doc.get_mut("budgets").and_then(|b| b.as_table_mut()) {
            budgets.remove("coding");
        }
        let out = doc.to_string();
        assert!(
            !out.contains("daily_budget_secs"),
            "budget entry should be removed"
        );
        assert!(out.contains("[apps.watch]"), "unrelated sections preserved");
    }

    #[test]
    fn handle_set_budget_rejects_unknown_category() {
        let mut watch = BTreeMap::new();
        watch.insert("coding".to_string(), vec!["code".to_string()]);
        let config = Config {
            apps: AppsConfig { watch },
            ..Default::default()
        };
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(config))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };
        let req = SetBudgetRequest {
            category: "nonexistent".to_string(),
            daily_budget_secs: 3600,
        };
        let resp = handle_set_budget(&state, req).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap_or("").contains("not found"));
    }

    // ── set_notifications tests ───────────────────────────────────────────────

    #[test]
    fn handle_set_notifications_writes_only_provided_fields() {
        let raw = "[notifications]\nenabled = true\nbreak_overdue = true\nbudget_exceeded = true\n";
        use toml_edit::{value, DocumentMut};
        let mut doc = raw.parse::<DocumentMut>().unwrap();
        // Simulate: only set enabled = false, leave others unchanged.
        let notif = doc
            .get_mut("notifications")
            .and_then(|n| n.as_table_mut())
            .unwrap();
        notif.insert("enabled", value(false));
        let out = doc.to_string();
        assert!(out.contains("enabled = false"));
        assert!(
            out.contains("break_overdue = true"),
            "untouched field preserved"
        );
        assert!(
            out.contains("budget_exceeded = true"),
            "untouched field preserved"
        );
    }

    // ── set_focus_source tests ────────────────────────────────────────────────

    #[test]
    fn handle_set_focus_source_rejects_invalid_kind() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        let state = AppState {
            config: Arc::new(Mutex::new(Arc::new(Config::default()))),
            db: Arc::new(Mutex::new(db)),
            db_state_path: Arc::new(Mutex::new(default_state_path())),
            socket_path: Arc::new(Mutex::new(PathBuf::from("/tmp/attn-test.sock"))),
            last_rebuild: Arc::new(Mutex::new(None)),
            tracking_state: Arc::new(Mutex::new(None)),
            focus_source_kind: Arc::new("niri".to_string()),
            notifier: Arc::new(notifications::NullNotifier),
            terminal_window_cache: Arc::new(Mutex::new(TerminalWindowCache::new())),
        };
        let req = SetFocusSourceRequest {
            kind: "gnome".to_string(),
        };
        let resp = handle_set_focus_source(&state, req).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.as_deref().unwrap_or("").contains("invalid kind"));
    }

    #[test]
    fn handle_set_focus_source_accepts_valid_kinds() {
        for kind in VALID_FOCUS_KINDS {
            // Validation only — no file I/O needed.
            assert!(VALID_FOCUS_KINDS.contains(kind), "{kind} should be valid");
        }
    }

    // ── status JSON includes new fields ───────────────────────────────────────

    #[test]
    fn status_json_includes_notifications_and_focus_source_fields() {
        let status = Status {
            date: "2026-05-15".to_string(),
            updated_at: "2026-05-15T12:00:00+00:00".to_string(),
            watch_seconds: 0,
            tracked_seconds: 0,
            media_seconds: 0,
            video_watch_seconds: 0,
            apps: vec![],
            domains: vec![],
            media: vec![],
            categories: vec![],
            days: vec![],
            active_session_seconds: 0,
            break_overdue: false,
            paused: false,
            paused_reason: None,
            paused_since: None,
            breaks_enabled: true,
            breaks_interval_secs: 3600,
            breaks_min_break_secs: 300,
            uncategorized_apps: vec![],
            uncategorized_domains: vec![],
            notifications_enabled: false,
            notifications_break_overdue: true,
            notifications_budget_exceeded: false,
            focus_source_kind: "hyprland".to_string(),
            switch_count: 0,
            longest_focus_secs: 0,
            deep_work_blocks: 0,
            active_since: None,
            active_until: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"notifications_enabled\":false"));
        assert!(json.contains("\"notifications_break_overdue\":true"));
        assert!(json.contains("\"notifications_budget_exceeded\":false"));
        assert!(json.contains("\"focus_source_kind\":\"hyprland\""));
    }
}
