use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use url::Url;

use super::{BrowserReader, BrowserVisit};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn visits_are_capped_at_next_visit_start() {
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
}

const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;

pub struct ChromiumReader;

impl BrowserReader for ChromiumReader {
    fn kind(&self) -> &'static str {
        "chromium"
    }

    fn read_visits(
        &self,
        profile_db: &Path,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<BrowserVisit>> {
        let db = Connection::open(profile_db).with_context(|| {
            format!(
                "failed to open chromium history snapshot {}",
                profile_db.display()
            )
        })?;
        let chrome_start = utc_to_chrome_micros(since - Duration::hours(12));
        let chrome_end = utc_to_chrome_micros(until + Duration::hours(12));
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
            if ended_at <= since || started_at >= until {
                continue;
            }
            visits.push(BrowserVisit {
                started_at,
                ended_at,
                url: raw_url,
                domain,
                source_profile: source_profile_from_path(profile_db),
            });
        }
        Ok(cap_visits_at_next_start(visits))
    }
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

fn source_profile_from_path(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("unknown")
        .to_string()
}

pub(crate) fn cap_visits_at_next_start(mut visits: Vec<BrowserVisit>) -> Vec<BrowserVisit> {
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
