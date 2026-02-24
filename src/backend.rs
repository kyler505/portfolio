use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::{header::LOCATION, redirect::Policy};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::lookup_host,
    sync::RwLock,
    time::{timeout, Instant},
};
use tower_http::services::{ServeDir, ServeFile};
use url::{Host, Url};

const PREVIEW_CACHE_TTL_SECONDS: u64 = 300;
const PREVIEW_CACHE_MAX_ENTRIES: usize = 256;
const RESPONSE_MAX_BYTES: usize = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REDIRECTS: usize = 4;
const MAX_RESOLVED_IP_ATTEMPTS: usize = 3;
const USER_AGENT: &str = "portfolio-preview-bot/1.0";

#[derive(Clone)]
pub struct AppState {
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

#[derive(Clone)]
struct CacheEntry {
    created_at: Instant,
    expires_at: Instant,
    value: PreviewPayload,
}

#[derive(Deserialize)]
struct PreviewQuery {
    url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewPayload {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl PreviewPayload {
    fn error(message: &str) -> Self {
        Self {
            ok: false,
            url: None,
            title: None,
            description: None,
            image: None,
            error: Some(message.to_string()),
        }
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);
    let bind_address = format!("0.0.0.0:{port}");

    let state = AppState {
        cache: Arc::new(RwLock::new(HashMap::new())),
    };

    let static_service = ServeDir::new("dist").not_found_service(ServeFile::new("dist/index.html"));

    let app = Router::new()
        .route("/api/preview", get(get_preview))
        .fallback_service(static_service)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
    println!("server listening on http://127.0.0.1:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_preview(
    State(state): State<AppState>,
    Query(query): Query<PreviewQuery>,
) -> impl IntoResponse {
    let parsed_url = match parse_preview_url(&query.url).await {
        Ok(url) => url,
        Err(error_message) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                PreviewPayload::error(error_message),
                cache_control("no-store"),
            )
        }
    };

    let normalized_url = parsed_url.to_string();

    if let Some(payload) = read_from_cache(&state, &normalized_url).await {
        return json_response(
            StatusCode::OK,
            payload,
            cache_control(&format!("public, max-age={PREVIEW_CACHE_TTL_SECONDS}")),
        );
    }

    let fetched = match fetch_preview_payload(parsed_url).await {
        Ok(payload) => payload,
        Err(error_message) => {
            return json_response(
                StatusCode::BAD_GATEWAY,
                PreviewPayload::error(error_message),
                cache_control("no-store"),
            )
        }
    };

    write_to_cache(&state, normalized_url, fetched.clone()).await;

    json_response(
        StatusCode::OK,
        fetched,
        cache_control(&format!("public, max-age={PREVIEW_CACHE_TTL_SECONDS}")),
    )
}

fn json_response(status: StatusCode, payload: PreviewPayload, cache_control: HeaderValue) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, cache_control);
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    (status, headers, Json(payload))
}

fn cache_control(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static("no-store"))
}

async fn read_from_cache(state: &AppState, key: &str) -> Option<PreviewPayload> {
    let now = Instant::now();
    {
        let cache = state.cache.read().await;
        let entry = cache.get(key)?;

        if entry.expires_at > now {
            return Some(entry.value.clone());
        }
    }

    let mut cache = state.cache.write().await;
    purge_expired_entries(&mut cache, now);
    cache.remove(key);
    None
}

async fn write_to_cache(state: &AppState, key: String, value: PreviewPayload) {
    let now = Instant::now();
    let mut cache = state.cache.write().await;

    purge_expired_entries(&mut cache, now);

    if !cache.contains_key(&key) && cache.len() >= PREVIEW_CACHE_MAX_ENTRIES {
        evict_oldest_entry(&mut cache);
    }

    cache.insert(
        key,
        CacheEntry {
            created_at: now,
            expires_at: now + Duration::from_secs(PREVIEW_CACHE_TTL_SECONDS),
            value,
        },
    );
}

fn purge_expired_entries(cache: &mut HashMap<String, CacheEntry>, now: Instant) {
    cache.retain(|_, entry| entry.expires_at > now);
}

fn evict_oldest_entry(cache: &mut HashMap<String, CacheEntry>) {
    let Some(key_to_remove) = cache
        .iter()
        .min_by_key(|(_, entry)| entry.created_at)
        .map(|(key, _)| key.clone())
    else {
        return;
    };

    cache.remove(&key_to_remove);
}

async fn parse_preview_url(raw_url: &str) -> Result<Url, &'static str> {
    let parsed = Url::parse(raw_url).map_err(|_| "invalid URL")?;

    ensure_url_shape_is_allowed(&parsed)?;
    Ok(parsed)
}

fn ensure_url_shape_is_allowed(url: &Url) -> Result<(), &'static str> {
    if !(url.scheme() == "http" || url.scheme() == "https") {
        return Err("URL scheme must be http or https");
    }

    let host = url.host_str().ok_or("URL host is required")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("local addresses are not allowed");
    }

    match url.host() {
        Some(Host::Ipv4(ipv4)) => {
            if is_disallowed_ip(IpAddr::V4(ipv4)) {
                return Err("host address is blocked");
            }
        }
        Some(Host::Ipv6(ipv6)) => {
            if is_disallowed_ip(IpAddr::V6(ipv6)) {
                return Err("host address is blocked");
            }
        }
        _ => {}
    }

    Ok(())
}

fn is_disallowed_ip(ip: IpAddr) -> bool {
    match normalize_ip_for_policy(ip) {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
                || is_documentation_ipv6(v6)
        }
    }
}

fn normalize_ip_for_policy(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        IpAddr::V4(v4) => IpAddr::V4(v4),
    }
}

fn is_documentation_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

async fn fetch_preview_payload(target_url: Url) -> Result<PreviewPayload, &'static str> {
    let mut current_url = target_url;

    for hop in 0..=MAX_REDIRECTS {
        let response = send_pinned_request(&current_url).await?;

        if response.status().is_redirection() {
            if hop == MAX_REDIRECTS {
                return Err("too many redirects");
            }

            let next_url = parse_and_validate_redirect_target(&current_url, &response).await?;
            current_url = next_url;
            continue;
        }

        if !response.status().is_success() {
            return Err("received non-success response");
        }

        let body = read_limited_body(response).await?;
        let metadata = extract_metadata(&body, &current_url);

        return Ok(PreviewPayload {
            ok: true,
            url: Some(current_url.to_string()),
            title: metadata.title,
            description: metadata.description,
            image: metadata.image,
            error: None,
        });
    }

    Err("too many redirects")
}

async fn parse_and_validate_redirect_target(
    current_url: &Url,
    response: &reqwest::Response,
) -> Result<Url, &'static str> {
    let location = response
        .headers()
        .get(LOCATION)
        .ok_or("received redirect without location")?;
    let location_value = location
        .to_str()
        .map_err(|_| "received invalid redirect location")?;
    let next_url = current_url
        .join(location_value)
        .map_err(|_| "received invalid redirect location")?;

    ensure_url_shape_is_allowed(&next_url)?;
    Ok(next_url)
}

async fn send_pinned_request(target_url: &Url) -> Result<reqwest::Response, &'static str> {
    ensure_url_shape_is_allowed(target_url)?;

    let host = target_url.host_str().ok_or("URL host is required")?;
    let host_port = target_url.port_or_known_default().ok_or("URL port is required")?;

    if host.parse::<IpAddr>().is_ok() {
        let client = build_preview_client(None)?;
        return client
            .get(target_url.clone())
            .send()
            .await
            .map_err(|_| "failed to fetch URL");
    }

    let resolved_ips = resolve_and_validate_host(host, host_port).await?;

    for pinned_ip in resolved_ips.into_iter().take(MAX_RESOLVED_IP_ATTEMPTS) {
        let pinned_socket = SocketAddr::new(pinned_ip, host_port);
        let client = build_preview_client(Some((host, pinned_socket)))?;

        match client.get(target_url.clone()).send().await {
            Ok(response) => return Ok(response),
            Err(_) => continue,
        }
    }

    Err("failed to fetch URL")
}

fn build_preview_client(
    resolved_host: Option<(&str, SocketAddr)>,
) -> Result<reqwest::Client, &'static str> {
    let mut client_builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT);

    if let Some((host, pinned_socket)) = resolved_host {
        client_builder = client_builder.resolve(host, pinned_socket);
    }

    client_builder
        .build()
        .map_err(|_| "failed to prepare request client")
}

async fn resolve_and_validate_host(host: &str, port: u16) -> Result<Vec<IpAddr>, &'static str> {
    let resolved_hosts = timeout(DNS_LOOKUP_TIMEOUT, lookup_host((host, port)))
        .await
        .map_err(|_| "host lookup timed out")?
        .map_err(|_| "unable to resolve host")?;

    collect_validated_resolved_ips(resolved_hosts)
}

fn collect_validated_resolved_ips(
    resolved_hosts: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<IpAddr>, &'static str> {
    let mut selected_ips: Vec<IpAddr> = Vec::new();
    let mut seen_ips: HashSet<IpAddr> = HashSet::new();

    for socket in resolved_hosts {
        let ip = socket.ip();

        if is_disallowed_ip(ip) {
            return Err("host address is blocked");
        }

        if seen_ips.insert(ip) {
            selected_ips.push(ip);
        }
    }

    if selected_ips.is_empty() {
        return Err("unable to resolve host");
    }

    Ok(selected_ips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn redirect_target_resolves_relative_location() {
        let current = Url::parse("http://93.184.216.34/start").expect("valid URL");

        let redirected = current
            .join("/next")
            .expect("relative redirect resolves");
        ensure_url_shape_is_allowed(&redirected).expect("public redirect target should be allowed");
    }

    #[test]
    fn blocked_private_target_is_rejected() {
        let blocked = Url::parse("http://127.0.0.1/private").expect("valid URL");

        let result = ensure_url_shape_is_allowed(&blocked);
        assert!(result.is_err());
    }

    #[test]
    fn blocked_ipv4_mapped_ipv6_target_is_rejected() {
        let blocked = Url::parse("http://[::ffff:127.0.0.1]/private").expect("valid URL");

        let result = ensure_url_shape_is_allowed(&blocked);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cache_overwrite_at_capacity_does_not_evict_oldest() {
        let state = AppState {
            cache: Arc::new(RwLock::new(HashMap::new())),
        };
        let now = Instant::now();

        {
            let mut cache = state.cache.write().await;

            for index in 0..PREVIEW_CACHE_MAX_ENTRIES {
                let key = format!("key-{index}");
                cache.insert(
                    key,
                    CacheEntry {
                        created_at: now + Duration::from_secs(index as u64),
                        expires_at: now + Duration::from_secs(10_000),
                        value: PreviewPayload {
                            ok: true,
                            url: Some("https://example.com".to_string()),
                            title: Some("title".to_string()),
                            description: None,
                            image: None,
                            error: None,
                        },
                    },
                );
            }
        }

        write_to_cache(
            &state,
            "key-10".to_string(),
            PreviewPayload {
                ok: true,
                url: Some("https://example.com/updated".to_string()),
                title: Some("updated".to_string()),
                description: None,
                image: None,
                error: None,
            },
        )
        .await;

        let cache = state.cache.read().await;
        assert_eq!(cache.len(), PREVIEW_CACHE_MAX_ENTRIES);
        assert!(cache.contains_key("key-0"));
        assert_eq!(
            cache.get("key-10").and_then(|entry| entry.value.title.as_deref()),
            Some("updated")
        );
    }

    #[test]
    fn collect_validated_resolved_ips_returns_multiple_unique_public_ips() {
        let resolved = vec![
            SocketAddr::new("93.184.216.34".parse().expect("valid ip"), 80),
            SocketAddr::new("2606:2800:220:1:248:1893:25c8:1946".parse().expect("valid ip"), 80),
            SocketAddr::new("93.184.216.34".parse().expect("valid ip"), 80),
        ];

        let ips = collect_validated_resolved_ips(resolved).expect("public addresses should pass");
        assert_eq!(ips.len(), 2);
    }
}

async fn read_limited_body(response: reqwest::Response) -> Result<String, &'static str> {
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::with_capacity(8192);

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|_| "failed reading response body")?;

        if body.len() + chunk.len() > RESPONSE_MAX_BYTES {
            return Err("response body too large");
        }

        body.extend_from_slice(&chunk);
    }

    Ok(String::from_utf8_lossy(&body).to_string())
}

struct ExtractedMetadata {
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
}

fn extract_metadata(document_html: &str, base_url: &Url) -> ExtractedMetadata {
    let document = Html::parse_document(document_html);

    let title = first_non_empty(vec![
        meta_content(&document, "property", "og:title"),
        meta_content(&document, "name", "twitter:title"),
        document_title(&document),
    ]);

    let description = first_non_empty(vec![
        meta_content(&document, "property", "og:description"),
        meta_content(&document, "name", "twitter:description"),
        meta_content(&document, "name", "description"),
    ]);

    let image = first_non_empty(vec![
        meta_content(&document, "property", "og:image"),
        meta_content(&document, "name", "twitter:image"),
    ])
    .and_then(|value| resolve_maybe_relative_url(base_url, &value));

    ExtractedMetadata {
        title,
        description,
        image,
    }
}

fn document_title(document: &Html) -> Option<String> {
    let title_selector = Selector::parse("title").ok()?;
    let title_element = document.select(&title_selector).next()?;
    normalize_text(title_element.text().collect::<String>())
}

fn meta_content(document: &Html, attribute: &str, attribute_value: &str) -> Option<String> {
    let selector = Selector::parse("meta").ok()?;

    for element in document.select(&selector) {
        if !element
            .value()
            .attr(attribute)
            .is_some_and(|value| value.eq_ignore_ascii_case(attribute_value))
        {
            continue;
        }

        if let Some(content) = element.value().attr("content") {
            if let Some(cleaned) = normalize_text(content.to_string()) {
                return Some(cleaned);
            }
        }
    }

    None
}

fn first_non_empty(values: Vec<Option<String>>) -> Option<String> {
    values.into_iter().flatten().find(|value| !value.is_empty())
}

fn normalize_text(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let collapsed_whitespace = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(collapsed_whitespace)
}

fn resolve_maybe_relative_url(base_url: &Url, value: &str) -> Option<String> {
    if let Ok(absolute) = Url::parse(value) {
        return Some(absolute.to_string());
    }

    base_url.join(value).ok().map(|joined| joined.to_string())
}
