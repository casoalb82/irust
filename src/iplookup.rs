
use std::sync::OnceLock;

const FIELDS: &str = "status,message,query,country,regionName,city,isp,org,as,lat,lon,timezone,proxy,hosting";

/// Built once and reused for every call: a fresh `reqwest::Client`
/// rebuilds its TLS config (the whole system CA bundle) from scratch
/// every time, and `Client` is intentionally cheap to clone/share (an
/// `Arc` internally).
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

pub async fn lookup(address: &str) -> Option<String> {
    let url = format!("http://ip-api.com/json/{address}?fields={FIELDS}");
    let resp = client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let parsed: serde_json::Value = resp.json().await.ok()?;
    if parsed.get("status").and_then(|v| v.as_str()) != Some("success") {
        return None;
    }

    let field = |key: &str| parsed.get(key).and_then(|v| v.as_str()).unwrap_or("?");
    let flag = |key: &str| {
        if parsed.get(key).and_then(|v| v.as_bool()).unwrap_or(false) {
            "yes"
        } else {
            "no"
        }
    };
    let query = field("query");
    let city = field("city");
    let region = field("regionName");
    let country = field("country");
    let isp = field("isp");
    let org = field("org");
    let asn = field("as");
    let lat = parsed
        .get("lat")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".to_string());
    let lon = parsed
        .get("lon")
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".to_string());
    let tz = field("timezone");
    let proxy = flag("proxy");
    let hosting = flag("hosting");

    Some(format!(
        "-- {city}, {region}, {country} · {isp} ({org}) · {asn} · {lat},{lon} · {tz} \
         · VPN/proxy: {proxy} · Hosting/datacenter: {hosting} · {query}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_address_fails_open_without_panicking() {
        // No network access assumed available in CI/sandboxes -- this
        // just confirms the function returns cleanly either way rather
        // than unwrapping something that isn't there.
        let _ = lookup("").await;
    }
}
