use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::Path;

pub mod chromium;
pub mod firefox;

#[derive(Debug)]
pub struct BrowserVisit {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub url: String,
    pub domain: String,
    pub source_profile: String,
}

pub trait BrowserReader {
    #[allow(dead_code)]
    fn kind(&self) -> &'static str;
    fn read_visits(
        &self,
        profile_db: &Path,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<BrowserVisit>>;
}

pub fn for_kind(kind: &str) -> Option<Box<dyn BrowserReader>> {
    match kind {
        "chromium" => Some(Box::new(chromium::ChromiumReader)),
        "firefox" => Some(Box::new(firefox::FirefoxReader)),
        _ => None,
    }
}
