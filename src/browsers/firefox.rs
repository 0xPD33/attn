use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use url::Url;

use super::{BrowserReader, BrowserVisit};

pub struct FirefoxReader;

impl BrowserReader for FirefoxReader {
    fn kind(&self) -> &'static str {
        "firefox"
    }

    fn read_visits(
        &self,
        profile_db: &Path,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<BrowserVisit>> {
        let db = Connection::open(profile_db).with_context(|| {
            format!(
                "failed to open Firefox places snapshot {}",
                profile_db.display()
            )
        })?;

        // PRTime is microseconds since Unix epoch — no offset needed.
        // Add a 12-hour buffer on each side to match chromium reader behaviour
        // (visits whose recorded time straddles the window edge are not lost).
        let prtime_start = since.timestamp_micros() - 12 * 3_600 * 1_000_000_i64;
        let prtime_end = until.timestamp_micros() + 12 * 3_600 * 1_000_000_i64;

        let mut stmt = db.prepare(
            "SELECT moz_historyvisits.visit_date,
                    0 AS visit_duration,
                    moz_places.url
             FROM moz_historyvisits
             JOIN moz_places ON moz_places.id = moz_historyvisits.place_id
             WHERE moz_historyvisits.visit_date >= ?1
               AND moz_historyvisits.visit_date <= ?2
             ORDER BY moz_historyvisits.visit_date ASC",
        )?;

        let rows = stmt.query_map(params![prtime_start, prtime_end], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let source_profile = source_profile_from_path(profile_db);
        let mut visits = Vec::new();
        for row in rows {
            let (prtime, _duration, raw_url) = row?;
            let Some(domain) = domain_from_url(&raw_url) else {
                continue;
            };
            let started_at = prtime_to_utc(prtime)?;
            // Firefox does not record visit duration. Use 0 — the downstream
            // insert_attributed_domain_intervals clips each visit to the focus
            // window containing its timestamp, so this is safe.
            let ended_at = started_at;
            if started_at < since || started_at >= until {
                continue;
            }
            visits.push(BrowserVisit {
                started_at,
                ended_at,
                url: raw_url,
                domain,
                source_profile: source_profile.clone(),
            });
        }
        Ok(visits)
    }
}

fn prtime_to_utc(prtime: i64) -> Result<DateTime<Utc>> {
    Utc.timestamp_micros(prtime)
        .single()
        .ok_or_else(|| anyhow!("invalid Firefox PRTime {prtime}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rusqlite::Connection;
    use std::path::PathBuf;

    fn build_fixture_db(path: &PathBuf) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE moz_places (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL,
                title TEXT
             );
             CREATE TABLE moz_historyvisits (
                id INTEGER PRIMARY KEY,
                place_id INTEGER NOT NULL,
                visit_date INTEGER NOT NULL,
                visit_type INTEGER
             );",
        )
        .unwrap();

        // visit_date is PRTime: microseconds since Unix epoch
        // 2026-05-11 10:00:00 UTC = 1778493600 seconds = 1778493600000000 µs
        let base_prtime: i64 = 1_778_493_600_000_000;

        db.execute(
            "INSERT INTO moz_places(id, url, title) VALUES (1, 'https://example.com/', 'Example')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO moz_places(id, url, title) VALUES (2, 'https://rust-lang.org/', 'Rust')",
            [],
        )
        .unwrap();

        db.execute(
            "INSERT INTO moz_historyvisits(id, place_id, visit_date, visit_type) VALUES (1, 1, ?1, 1)",
            params![base_prtime],
        )
        .unwrap();
        db.execute(
            "INSERT INTO moz_historyvisits(id, place_id, visit_date, visit_type) VALUES (2, 2, ?1, 1)",
            params![base_prtime + 60_000_000], // 60 seconds later
        )
        .unwrap();
    }

    #[test]
    fn firefox_reader_returns_expected_visits() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "attn-firefox-fixture-{}.sqlite",
            std::process::id()
        ));
        build_fixture_db(&path);

        let reader = FirefoxReader;
        let since = Utc.with_ymd_and_hms(2026, 5, 11, 9, 59, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 5, 11, 10, 5, 0).unwrap();
        let visits = reader.read_visits(&path, since, until).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(visits.len(), 2, "expected 2 visits from fixture");

        let v0 = &visits[0];
        assert_eq!(v0.domain, "example.com");
        assert_eq!(
            v0.started_at,
            Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap()
        );
        assert_eq!(v0.started_at, v0.ended_at, "duration should be 0 for firefox");

        let v1 = &visits[1];
        assert_eq!(v1.domain, "rust-lang.org");
        assert_eq!(
            v1.started_at,
            Utc.with_ymd_and_hms(2026, 5, 11, 10, 1, 0).unwrap()
        );
    }

    #[test]
    fn firefox_reader_filters_outside_window() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "attn-firefox-filter-{}.sqlite",
            std::process::id()
        ));
        build_fixture_db(&path);

        let reader = FirefoxReader;
        // Window that excludes both visits
        let since = Utc.with_ymd_and_hms(2026, 5, 11, 11, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        let visits = reader.read_visits(&path, since, until).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(visits.is_empty(), "visits outside window should be filtered");
    }
}
