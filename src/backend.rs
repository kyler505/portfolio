use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};
use tokio::{net::lookup_host, sync::RwLock, time::Instant};
use tower_http::services::{ServeDir, ServeFile};
use url::Url;

const PREVIEW_CACHE_TTL_SECONDS: u64 = 300;
const RESPONSE_MAX_BYTES: usize = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REDIRECTS: usize = 4;
const USER_AGENT: &str = "portfolio-preview-bot/1.0";

#[derive(Clone)]
pub struct AppState {
    http_client: reqwest::Client,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

#[derive(Clone)]
struct CacheEntry {
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
        http_client: reqwest::Client::builder()
            .redirect(Policy::limited(MAX_REDIRECTS))
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()?,
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

    let fetched = match fetch_preview_payload(&state.http_client, parsed_url).await {
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
    let cache = state.cache.read().await;
    let entry = cache.get(key)?;

    if entry.expires_at <= now {
        return None;
    }

    Some(entry.value.clone())
}

async fn write_to_cache(state: &AppState, key: String, value: PreviewPayload) {
    let mut cache = state.cache.write().await;
    cache.insert(
        key,
        CacheEntry {
            expires_at: Instant::now() + Duration::from_secs(PREVIEW_CACHE_TTL_SECONDS),
            value,
        },
    );
}

async fn parse_preview_url(raw_url: &str) -> Result<Url, &'static str> {
    let parsed = Url::parse(raw_url).map_err(|_| "invalid URL")?;

    if !(parsed.scheme() == "http" || parsed.scheme() == "https") {
        return Err("URL scheme must be http or https");
    }

    let host = parsed.host_str().ok_or("URL host is required")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("local addresses are not allowed");
    }

    ensure_host_is_safe(host, parsed.port_or_known_default()).await?;
    Ok(parsed)
}

async fn ensure_host_is_safe(host: &str, port: Option<u16>) -> Result<(), &'static str> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_disallowed_ip(ip) {
            return Err("host address is blocked");
        }
        return Ok(());
    }

    let resolved_port = port.unwrap_or(80);
    let resolved_hosts = lookup_host((host, resolved_port))
        .await
        .map_err(|_| "unable to resolve host")?;

    for socket in resolved_hosts {
        if is_disallowed_ip(socket.ip()) {
            return Err("host address is blocked");
        }
    }

    Ok(())
}

fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
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

fn is_documentation_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

async fn fetch_preview_payload(
    client: &reqwest::Client,
    target_url: Url,
) -> Result<PreviewPayload, &'static str> {
    let response = client
        .get(target_url)
        .send()
        .await
        .map_err(|_| "failed to fetch URL")?;

    if !response.status().is_success() {
        return Err("received non-success response");
    }

    let final_url = response.url().clone();
    if let Some(host) = final_url.host_str() {
        ensure_host_is_safe(host, final_url.port_or_known_default()).await?;
    }

    let body = read_limited_body(response).await?;
    let metadata = extract_metadata(&body, &final_url);

    Ok(PreviewPayload {
        ok: true,
        url: Some(final_url.to_string()),
        title: metadata.title,
        description: metadata.description,
        image: metadata.image,
        error: None,
    })
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
