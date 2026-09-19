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

pub(crate) fn normalize_visit_ranges(
    mut visits: Vec<BrowserVisit>,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Vec<BrowserVisit> {
    visits.sort_by_key(|visit| visit.started_at);
    let starts = visits
        .iter()
        .map(|visit| visit.started_at)
        .collect::<Vec<_>>();

    for (index, visit) in visits.iter_mut().enumerate() {
        // Chromium stores redirect hops with one shared timestamp. The
        // successor is the next strictly later start, never a same-instant
        // sibling; otherwise a zero-length hop extends to `until`.
        let next_start = starts[index + 1..]
            .iter()
            .copied()
            .find(|next| *next > visit.started_at);
        if visit.ended_at <= visit.started_at {
            visit.ended_at = next_start.unwrap_or_else(|| {
                if visit.started_at >= since {
                    until
                } else {
                    visit.started_at
                }
            });
        } else if let Some(next) = next_start {
            if next < visit.ended_at {
                visit.ended_at = next;
            }
        }
    }

    visits
        .into_iter()
        .filter(|visit| visit.ended_at > visit.started_at)
        .filter(|visit| visit.ended_at > since && visit.started_at < until)
        .collect()
}
