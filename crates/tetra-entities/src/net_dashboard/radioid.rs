// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const RADIOID_API: &str = "https://radioid.net/api/users";
const USER_AGENT: &str = "Nexus-BS/dashboard-radioid";
const POSITIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const NEGATIVE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FAILURE_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(2500);
const CACHE_MAX: usize = 500;

#[derive(Clone)]
struct CachedLookup {
    body: String,
    fetched_at: Instant,
    ttl: Duration,
}

#[derive(Default)]
struct RadioIdCache {
    entries: HashMap<u32, CachedLookup>,
    failures: HashMap<u32, Instant>,
    inflight: HashSet<u32>,
    fetching: bool,
    last_request: Option<Instant>,
}

static CACHE: OnceLock<Mutex<RadioIdCache>> = OnceLock::new();

fn cache() -> &'static Mutex<RadioIdCache> {
    CACHE.get_or_init(|| Mutex::new(RadioIdCache::default()))
}

/// Serve a same-origin RadioID lookup for the dashboard.
///
/// This deliberately runs under the dashboard HTTP worker, with cache and
/// one-at-a-time outbound fetches, so browser CORS problems and RadioID latency
/// do not touch the RF/CMCE/UMAC stack loop.
pub fn serve(stream: TcpStream, issi: Option<u32>) {
    let Some(issi) = issi.filter(|issi| valid_issi(*issi)) else {
        return json(stream, r#"{"ok":false,"error":"invalid ISSI"}"#);
    };

    match lookup(issi) {
        LookupResult::Ready(body) => json(stream, &body),
        LookupResult::Pending => json(stream, r#"{"ok":false,"pending":true}"#),
        LookupResult::Unavailable => json(stream, r#"{"ok":false,"unavailable":true}"#),
    }
}

enum LookupResult {
    Ready(String),
    Pending,
    Unavailable,
}

fn valid_issi(issi: u32) -> bool {
    issi > 0 && issi < 0x00ff_ffff
}

fn lookup(issi: u32) -> LookupResult {
    let wait = {
        let Ok(mut cache) = cache().lock() else {
            return LookupResult::Unavailable;
        };
        prune_locked(&mut cache);
        if let Some(entry) = cache.entries.get(&issi) {
            if entry.fetched_at.elapsed() <= entry.ttl {
                return LookupResult::Ready(entry.body.clone());
            }
        }
        if cache.failures.get(&issi).is_some_and(|until| *until > Instant::now()) {
            return LookupResult::Unavailable;
        }
        if cache.fetching || cache.inflight.contains(&issi) {
            return LookupResult::Pending;
        }

        cache.fetching = true;
        cache.inflight.insert(issi);
        cache
            .last_request
            .and_then(|last| MIN_REQUEST_INTERVAL.checked_sub(last.elapsed()))
            .unwrap_or(Duration::ZERO)
    };

    if !wait.is_zero() {
        std::thread::sleep(wait);
    }

    let result = fetch_radioid(issi);

    let Ok(mut cache) = cache().lock() else {
        return LookupResult::Unavailable;
    };
    cache.fetching = false;
    cache.inflight.remove(&issi);
    cache.last_request = Some(Instant::now());

    match result {
        Ok((body, found)) => {
            let ttl = if found { POSITIVE_TTL } else { NEGATIVE_TTL };
            cache.entries.insert(
                issi,
                CachedLookup {
                    body: body.clone(),
                    fetched_at: Instant::now(),
                    ttl,
                },
            );
            prune_locked(&mut cache);
            LookupResult::Ready(body)
        }
        Err(_) => {
            cache.failures.insert(issi, Instant::now() + FAILURE_BACKOFF);
            LookupResult::Unavailable
        }
    }
}

fn fetch_radioid(issi: u32) -> Result<(String, bool), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;

    let payload: serde_json::Value = client
        .get(RADIOID_API)
        .query(&[
            ("id", issi.to_string()),
            ("id_sel", "=".to_string()),
            ("page", "1".to_string()),
            ("per_page", "1".to_string()),
        ])
        .send()
        .and_then(|resp| resp.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;

    Ok(normalize_payload(issi, &payload))
}

fn normalize_payload(issi: u32, payload: &serde_json::Value) -> (String, bool) {
    let row = payload.get("results").and_then(|results| results.as_array()).and_then(|rows| {
        rows.iter()
            .find(|row| field_u32(row, "id") == Some(issi) || field_u32(row, "radio_id") == Some(issi))
            .or_else(|| rows.first())
    });

    let Some(row) = row else {
        return (format!(r#"{{"ok":true,"issi":{issi},"missing":true}}"#), false);
    };

    let callsign = field_str(row, "callsign").unwrap_or_default().trim().to_ascii_uppercase();
    if callsign.is_empty() {
        return (format!(r#"{{"ok":true,"issi":{issi},"missing":true}}"#), false);
    }

    let name = field_str(row, "name").filter(|name| !name.trim().is_empty()).unwrap_or_else(|| {
        let first = field_str(row, "fname").unwrap_or_default();
        let last = field_str(row, "surname").unwrap_or_default();
        format!("{} {}", first.trim(), last.trim()).trim().to_string()
    });
    let country = field_str(row, "country").unwrap_or_default().trim().to_string();

    (
        serde_json::json!({
            "ok": true,
            "issi": issi,
            "callsign": callsign,
            "name": name,
            "country": country,
            "missing": false,
        })
        .to_string(),
        true,
    )
}

fn field_str(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key).and_then(|value| value.as_str()).map(str::to_string)
}

fn field_u32(row: &serde_json::Value, key: &str) -> Option<u32> {
    row.get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
}

fn prune_locked(cache: &mut RadioIdCache) {
    let now = Instant::now();
    cache.entries.retain(|_, entry| entry.fetched_at + entry.ttl > now);
    cache.failures.retain(|_, until| *until > now);
    if cache.entries.len() <= CACHE_MAX {
        return;
    }

    let mut keys: Vec<_> = cache.entries.iter().map(|(issi, entry)| (*issi, entry.fetched_at)).collect();
    keys.sort_by_key(|(_, fetched_at)| *fetched_at);
    let remove_count = cache.entries.len().saturating_sub(CACHE_MAX);
    for (issi, _) in keys.into_iter().take(remove_count) {
        cache.entries.remove(&issi);
    }
}

fn json(mut stream: TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_radioid_payload() {
        let payload = serde_json::json!({
            "results": [{
                "id": 2260618,
                "callsign": "yo3tco",
                "country": "Romania",
                "fname": "Chris",
                "surname": "YO3TCO"
            }]
        });

        let (body, found) = normalize_payload(2260618, &payload);
        assert!(found);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["issi"], 2260618);
        assert_eq!(json["callsign"], "YO3TCO");
        assert_eq!(json["name"], "Chris YO3TCO");
        assert_eq!(json["country"], "Romania");
    }

    #[test]
    fn missing_radioid_payload_is_negative_cacheable() {
        let payload = serde_json::json!({ "results": [] });
        let (body, found) = normalize_payload(2260618, &payload);
        assert!(!found);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["missing"], true);
    }
}
