use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::{
    header::{AUTHORIZATION, LOCATION},
    redirect::Policy,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::lookup_host,
    sync::{RwLock, Semaphore},
    time::{timeout, Instant},
};
use tower_http::services::{ServeDir, ServeFile};
use url::{Host, Url};

const DEFAULT_PREVIEW_CACHE_TTL_SECONDS: u64 = 300;
const DEFAULT_PREVIEW_CACHE_MAX_ENTRIES: usize = 256;
const DEFAULT_PREVIEW_RESPONSE_MAX_BYTES: usize = 512 * 1024;
const DEFAULT_PREVIEW_REQUEST_TIMEOUT_MS: u64 = 6_000;
const DEFAULT_PREVIEW_CONNECT_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_PREVIEW_DNS_LOOKUP_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_PREVIEW_MAX_REDIRECTS: usize = 4;
const DEFAULT_PREVIEW_MAX_RESOLVED_IP_ATTEMPTS: usize = 3;
const DEFAULT_SCREENSHOT_WORKER_TIMEOUT_MS: u64 = 8_000;
const DEFAULT_SCREENSHOT_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const DEFAULT_SCREENSHOT_STALE_GRACE_SECONDS: u64 = 14 * 24 * 60 * 60;
const DEFAULT_SCREENSHOT_REFRESH_CONCURRENCY: usize = 3;
const DEFAULT_SCREENSHOT_CACHE_INDEX_PATH: &str = "/tmp/preview-cache.json";
const DEFAULT_SCREENSHOT_URL_LIST_PATH: &str = "config/preview-urls.json";

const PREVIEW_CACHE_TTL_SECONDS_BOUNDS: (u64, u64) = (1, 86_400);
const PREVIEW_CACHE_MAX_ENTRIES_BOUNDS: (usize, usize) = (1, 10_000);
const PREVIEW_RESPONSE_MAX_BYTES_BOUNDS: (usize, usize) = (1_024, 10 * 1024 * 1024);
const PREVIEW_REQUEST_TIMEOUT_MS_BOUNDS: (u64, u64) = (100, 120_000);
const PREVIEW_CONNECT_TIMEOUT_MS_BOUNDS: (u64, u64) = (100, 30_000);
const PREVIEW_DNS_LOOKUP_TIMEOUT_MS_BOUNDS: (u64, u64) = (100, 30_000);
const PREVIEW_MAX_REDIRECTS_BOUNDS: (usize, usize) = (1, 10);
const PREVIEW_MAX_RESOLVED_IP_ATTEMPTS_BOUNDS: (usize, usize) = (1, 10);
const SCREENSHOT_WORKER_TIMEOUT_MS_BOUNDS: (u64, u64) = (100, 120_000);
const SCREENSHOT_TTL_SECONDS_BOUNDS: (u64, u64) = (60, 365 * 24 * 60 * 60);
const SCREENSHOT_STALE_GRACE_SECONDS_BOUNDS: (u64, u64) = (0, 365 * 24 * 60 * 60);
const SCREENSHOT_REFRESH_CONCURRENCY_BOUNDS: (usize, usize) = (2, 4);
const USER_AGENT: &str = "portfolio-preview-bot/1.0";

#[derive(Clone)]
struct PreviewRuntimeConfig {
    cache_ttl_seconds: u64,
    cache_max_entries: usize,
    response_max_bytes: usize,
    request_timeout: Duration,
    connect_timeout: Duration,
    dns_lookup_timeout: Duration,
    max_redirects: usize,
    max_resolved_ip_attempts: usize,
    screenshot_worker_url: Option<Url>,
    screenshot_worker_timeout: Duration,
    screenshot_worker_token: Option<String>,
    screenshot_ttl_seconds: u64,
    screenshot_stale_grace_seconds: u64,
    screenshot_cache_index_path: PathBuf,
    screenshot_refresh_token: Option<String>,
    screenshot_refresh_concurrency: usize,
    screenshot_refresh_urls_path: PathBuf,
}

impl PreviewRuntimeConfig {
    fn from_env() -> Self {
        let cache_ttl_seconds = parse_env_u64_with_bounds(
            "PREVIEW_CACHE_TTL_SECONDS",
            DEFAULT_PREVIEW_CACHE_TTL_SECONDS,
            PREVIEW_CACHE_TTL_SECONDS_BOUNDS,
        );
        let cache_max_entries = parse_env_usize_with_bounds(
            "PREVIEW_CACHE_MAX_ENTRIES",
            DEFAULT_PREVIEW_CACHE_MAX_ENTRIES,
            PREVIEW_CACHE_MAX_ENTRIES_BOUNDS,
        );
        let response_max_bytes = parse_env_usize_with_bounds(
            "PREVIEW_RESPONSE_MAX_BYTES",
            DEFAULT_PREVIEW_RESPONSE_MAX_BYTES,
            PREVIEW_RESPONSE_MAX_BYTES_BOUNDS,
        );
        let request_timeout_ms = parse_timeout_ms_with_legacy_seconds(
            "PREVIEW_REQUEST_TIMEOUT_MS",
            "PREVIEW_REQUEST_TIMEOUT_SECONDS",
            DEFAULT_PREVIEW_REQUEST_TIMEOUT_MS,
            PREVIEW_REQUEST_TIMEOUT_MS_BOUNDS,
        );
        let connect_timeout_ms = parse_timeout_ms_with_legacy_seconds(
            "PREVIEW_CONNECT_TIMEOUT_MS",
            "PREVIEW_CONNECT_TIMEOUT_SECONDS",
            DEFAULT_PREVIEW_CONNECT_TIMEOUT_MS,
            PREVIEW_CONNECT_TIMEOUT_MS_BOUNDS,
        );
        let dns_lookup_timeout_ms = parse_timeout_ms_with_legacy_seconds(
            "PREVIEW_DNS_LOOKUP_TIMEOUT_MS",
            "PREVIEW_DNS_LOOKUP_TIMEOUT_SECONDS",
            DEFAULT_PREVIEW_DNS_LOOKUP_TIMEOUT_MS,
            PREVIEW_DNS_LOOKUP_TIMEOUT_MS_BOUNDS,
        );
        let max_redirects = parse_env_usize_with_bounds(
            "PREVIEW_MAX_REDIRECTS",
            DEFAULT_PREVIEW_MAX_REDIRECTS,
            PREVIEW_MAX_REDIRECTS_BOUNDS,
        );
        let max_resolved_ip_attempts = parse_env_usize_with_bounds(
            "PREVIEW_MAX_RESOLVED_IP_ATTEMPTS",
            DEFAULT_PREVIEW_MAX_RESOLVED_IP_ATTEMPTS,
            PREVIEW_MAX_RESOLVED_IP_ATTEMPTS_BOUNDS,
        );
        let screenshot_worker_timeout_ms = parse_env_u64_with_bounds(
            "SCREENSHOT_WORKER_TIMEOUT_MS",
            DEFAULT_SCREENSHOT_WORKER_TIMEOUT_MS,
            SCREENSHOT_WORKER_TIMEOUT_MS_BOUNDS,
        );
        let screenshot_ttl_seconds = parse_env_u64_with_bounds(
            "SCREENSHOT_TTL_SECONDS",
            DEFAULT_SCREENSHOT_TTL_SECONDS,
            SCREENSHOT_TTL_SECONDS_BOUNDS,
        );
        let screenshot_stale_grace_seconds = parse_env_u64_with_bounds(
            "SCREENSHOT_STALE_GRACE_SECONDS",
            DEFAULT_SCREENSHOT_STALE_GRACE_SECONDS,
            SCREENSHOT_STALE_GRACE_SECONDS_BOUNDS,
        );
        let screenshot_refresh_concurrency = parse_env_usize_with_bounds(
            "SCREENSHOT_REFRESH_CONCURRENCY",
            DEFAULT_SCREENSHOT_REFRESH_CONCURRENCY,
            SCREENSHOT_REFRESH_CONCURRENCY_BOUNDS,
        );
        let screenshot_worker_url = parse_env_http_url("SCREENSHOT_WORKER_URL");
        let screenshot_worker_token = parse_env_non_empty_string("SCREENSHOT_WORKER_TOKEN");
        let screenshot_cache_index_path = parse_env_non_empty_string("SCREENSHOT_CACHE_INDEX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SCREENSHOT_CACHE_INDEX_PATH));
        let screenshot_refresh_token = parse_env_non_empty_string("SCREENSHOT_REFRESH_TOKEN");
        let screenshot_refresh_urls_path = parse_env_non_empty_string("SCREENSHOT_URLS_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SCREENSHOT_URL_LIST_PATH));

        Self {
            cache_ttl_seconds,
            cache_max_entries,
            response_max_bytes,
            request_timeout: Duration::from_millis(request_timeout_ms),
            connect_timeout: Duration::from_millis(connect_timeout_ms),
            dns_lookup_timeout: Duration::from_millis(dns_lookup_timeout_ms),
            max_redirects,
            max_resolved_ip_attempts,
            screenshot_worker_url,
            screenshot_worker_timeout: Duration::from_millis(screenshot_worker_timeout_ms),
            screenshot_worker_token,
            screenshot_ttl_seconds,
            screenshot_stale_grace_seconds,
            screenshot_cache_index_path,
            screenshot_refresh_token,
            screenshot_refresh_concurrency,
            screenshot_refresh_urls_path,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    screenshot_cache: Arc<RwLock<ScreenshotCacheStore>>,
    screenshot_refresh_in_flight: Arc<RwLock<HashSet<String>>>,
    config: PreviewRuntimeConfig,
}

#[derive(Clone)]
struct CacheEntry {
    created_at: Instant,
    expires_at: Instant,
    value: PreviewPayload,
}

#[derive(Clone, Serialize, Deserialize)]
struct ScreenshotCacheEntry {
    image: String,
    captured_at: u64,
    expires_at: u64,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct ScreenshotCacheIndex {
    entries: HashMap<String, ScreenshotCacheEntry>,
}

struct ScreenshotCacheStore {
    file_path: PathBuf,
    entries: HashMap<String, ScreenshotCacheEntry>,
}

impl ScreenshotCacheStore {
    fn load_from_disk(file_path: PathBuf) -> Self {
        let entries = read_screenshot_cache_index(&file_path)
            .map(|index| index.entries)
            .unwrap_or_default();

        Self { file_path, entries }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScreenshotCacheDecision {
    Fresh,
    StaleWithinGrace,
    MissingOrExpired,
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
    let preview_config = PreviewRuntimeConfig::from_env();
    let screenshot_cache =
        ScreenshotCacheStore::load_from_disk(preview_config.screenshot_cache_index_path.clone());

    let state = AppState {
        cache: Arc::new(RwLock::new(HashMap::new())),
        screenshot_cache: Arc::new(RwLock::new(screenshot_cache)),
        screenshot_refresh_in_flight: Arc::new(RwLock::new(HashSet::new())),
        config: preview_config,
    };

    let static_service = ServeDir::new("dist").not_found_service(ServeFile::new("dist/index.html"));

    let app = Router::new()
        .route("/api/preview", get(get_preview))
        .route(
            "/internal/refresh-screenshots",
            post(refresh_screenshots_endpoint),
        )
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
            cache_control(&format!("public, max-age={}", state.config.cache_ttl_seconds)),
        );
    }

    let fetched = match fetch_preview_payload(parsed_url, &state).await {
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
        cache_control(&format!("public, max-age={}", state.config.cache_ttl_seconds)),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotRefreshSummary {
    ok: bool,
    requested_urls: usize,
    refreshed: usize,
    invalid: usize,
    failed: usize,
}

async fn refresh_screenshots_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.config.screenshot_refresh_token.is_none() {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            PreviewPayload::error("refresh token is not configured"),
            cache_control("no-store"),
        );
    }

    if !is_refresh_authorized(&headers, &state.config) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            PreviewPayload::error("unauthorized"),
            cache_control("no-store"),
        );
    }

    let raw_urls = match read_refresh_urls_from_config(&state.config.screenshot_refresh_urls_path) {
        Ok(urls) => urls,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                PreviewPayload::error("unable to read configured URL list"),
                cache_control("no-store"),
            )
        }
    };

    let mut valid_urls = Vec::new();
    let mut invalid = 0usize;

    for raw_url in &raw_urls {
        match parse_preview_url(raw_url).await {
            Ok(parsed) => valid_urls.push(parsed),
            Err(_) => invalid += 1,
        }
    }

    let semaphore = Arc::new(Semaphore::new(state.config.screenshot_refresh_concurrency));
    let mut tasks = futures_util::stream::FuturesUnordered::new();

    for url in valid_urls {
        let state_clone = state.clone();
        let semaphore_clone = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let Ok(_permit) = semaphore_clone.acquire_owned().await else {
                return false;
            };

            refresh_screenshot_for_url(&state_clone, &url, "scheduled-refresh")
                .await
                .is_some()
        }));
    }

    let mut refreshed = 0usize;
    let mut failed = 0usize;

    while let Some(join_result) = tasks.next().await {
        match join_result {
            Ok(true) => refreshed += 1,
            Ok(false) | Err(_) => failed += 1,
        }
    }

    let summary = ScreenshotRefreshSummary {
        ok: true,
        requested_urls: raw_urls.len(),
        refreshed,
        invalid,
        failed,
    };

    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CACHE_CONTROL, cache_control("no-store"));
    response_headers.insert(header::VARY, HeaderValue::from_static("Authorization"));
    (StatusCode::OK, response_headers, Json(summary)).into_response()
}

fn json_response(
    status: StatusCode,
    payload: PreviewPayload,
    cache_control: HeaderValue,
) -> axum::response::Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, cache_control);
    headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    (status, headers, Json(payload)).into_response()
}

fn cache_control(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static("no-store"))
}

fn parse_env_u64_with_bounds(name: &str, default: u64, bounds: (u64, u64)) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| (bounds.0..=bounds.1).contains(value))
        .unwrap_or(default)
}

fn parse_env_usize_with_bounds(name: &str, default: usize, bounds: (usize, usize)) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| (bounds.0..=bounds.1).contains(value))
        .unwrap_or(default)
}

fn parse_timeout_ms_with_legacy_seconds(
    milliseconds_key: &str,
    seconds_key: &str,
    default_ms: u64,
    bounds: (u64, u64),
) -> u64 {
    if let Some(milliseconds) = std::env::var(milliseconds_key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| (bounds.0..=bounds.1).contains(value))
    {
        return milliseconds;
    }

    std::env::var(seconds_key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .and_then(|seconds| seconds.checked_mul(1_000))
        .filter(|value| (bounds.0..=bounds.1).contains(value))
        .unwrap_or(default_ms)
}

fn parse_env_non_empty_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_env_http_url(name: &str) -> Option<Url> {
    let value = parse_env_non_empty_string(name)?;
    let parsed = Url::parse(&value).ok()?;

    if parsed.scheme() == "http" || parsed.scheme() == "https" {
        Some(parsed)
    } else {
        None
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn read_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let authorization = headers.get(AUTHORIZATION)?;
    let value = authorization.to_str().ok()?;
    let prefix = "Bearer ";

    if !value.starts_with(prefix) {
        return None;
    }

    Some(value[prefix.len()..].trim())
}

fn is_refresh_authorized(headers: &HeaderMap, config: &PreviewRuntimeConfig) -> bool {
    let Some(expected_token) = config.screenshot_refresh_token.as_deref() else {
        return false;
    };

    let Some(provided_token) = read_bearer_token(headers) else {
        return false;
    };

    !provided_token.is_empty() && provided_token == expected_token
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RefreshUrlConfig {
    Bare(Vec<String>),
    Wrapped { urls: Vec<String> },
}

impl RefreshUrlConfig {
    fn into_urls(self) -> Vec<String> {
        match self {
            Self::Bare(urls) => urls,
            Self::Wrapped { urls } => urls,
        }
    }
}

fn read_refresh_urls_from_config(path: &Path) -> Result<Vec<String>, ()> {
    let raw = fs::read_to_string(path).map_err(|_| ())?;
    let parsed: RefreshUrlConfig = serde_json::from_str(&raw).map_err(|_| ())?;

    let urls = parsed
        .into_urls()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    Ok(urls)
}

fn read_screenshot_cache_index(path: &Path) -> Result<ScreenshotCacheIndex, ()> {
    let raw = fs::read_to_string(path).map_err(|_| ())?;
    serde_json::from_str(&raw).map_err(|_| ())
}

fn write_screenshot_cache_index(path: &Path, entries: &HashMap<String, ScreenshotCacheEntry>) -> Result<(), ()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| ())?;
    }

    let index = ScreenshotCacheIndex {
        entries: entries.clone(),
    };
    let encoded = serde_json::to_vec_pretty(&index).map_err(|_| ())?;
    fs::write(path, encoded).map_err(|_| ())
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

    if !cache.contains_key(&key) && cache.len() >= state.config.cache_max_entries {
        evict_oldest_entry(&mut cache);
    }

    cache.insert(
        key,
        CacheEntry {
            created_at: now,
            expires_at: now + Duration::from_secs(state.config.cache_ttl_seconds),
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

fn decide_screenshot_cache_action(
    now_unix: u64,
    cache_entry: Option<&ScreenshotCacheEntry>,
    stale_grace_seconds: u64,
) -> ScreenshotCacheDecision {
    let Some(entry) = cache_entry else {
        return ScreenshotCacheDecision::MissingOrExpired;
    };

    if now_unix < entry.expires_at {
        return ScreenshotCacheDecision::Fresh;
    }

    let stale_limit = entry.expires_at.saturating_add(stale_grace_seconds);
    if now_unix <= stale_limit {
        return ScreenshotCacheDecision::StaleWithinGrace;
    }

    ScreenshotCacheDecision::MissingOrExpired
}

async fn read_screenshot_cache_entry(state: &AppState, key: &str) -> Option<ScreenshotCacheEntry> {
    let cache = state.screenshot_cache.read().await;
    cache.entries.get(key).cloned()
}

async fn write_screenshot_cache_entry(state: &AppState, key: String, entry: ScreenshotCacheEntry) {
    let (path, entries_snapshot) = {
        let mut cache = state.screenshot_cache.write().await;
        cache.entries.insert(key, entry);
        (cache.file_path.clone(), cache.entries.clone())
    };

    let _ = write_screenshot_cache_index(&path, &entries_snapshot);
}

async fn update_screenshot_cache_error(state: &AppState, key: &str, message: &str) {
    let (path, entries_snapshot) = {
        let mut cache = state.screenshot_cache.write().await;
        if let Some(entry) = cache.entries.get_mut(key) {
            entry.last_error = Some(message.to_string());
        }

        (cache.file_path.clone(), cache.entries.clone())
    };

    let _ = write_screenshot_cache_index(&path, &entries_snapshot);
}

async fn refresh_screenshot_for_url(state: &AppState, target_url: &Url, source: &str) -> Option<String> {
    let captured_at = now_unix_seconds();
    let image = fetch_screenshot_image(target_url, &state.config).await;
    let key = target_url.to_string();

    match image {
        Some(image_value) => {
            let entry = ScreenshotCacheEntry {
                image: image_value.clone(),
                captured_at,
                expires_at: captured_at.saturating_add(state.config.screenshot_ttl_seconds),
                source: source.to_string(),
                last_error: None,
            };
            write_screenshot_cache_entry(state, key, entry).await;
            Some(image_value)
        }
        None => {
            update_screenshot_cache_error(state, &key, "failed to capture screenshot").await;
            None
        }
    }
}

async fn start_background_screenshot_refresh(state: AppState, target_url: Url) {
    let key = target_url.to_string();
    {
        let mut in_flight = state.screenshot_refresh_in_flight.write().await;
        if !in_flight.insert(key.clone()) {
            return;
        }
    }

    tokio::spawn(async move {
        let _ = refresh_screenshot_for_url(&state, &target_url, "async-stale-refresh").await;
        let mut in_flight = state.screenshot_refresh_in_flight.write().await;
        in_flight.remove(&key);
    });
}

async fn resolve_screenshot_image_for_preview(state: &AppState, target_url: &Url) -> Option<String> {
    let key = target_url.to_string();
    let cached = read_screenshot_cache_entry(state, &key).await;
    let now_unix = now_unix_seconds();

    match decide_screenshot_cache_action(
        now_unix,
        cached.as_ref(),
        state.config.screenshot_stale_grace_seconds,
    ) {
        ScreenshotCacheDecision::Fresh => cached.map(|entry| entry.image),
        ScreenshotCacheDecision::StaleWithinGrace => {
            if let Some(entry) = cached {
                start_background_screenshot_refresh(state.clone(), target_url.clone()).await;
                Some(entry.image)
            } else {
                None
            }
        }
        ScreenshotCacheDecision::MissingOrExpired => {
            refresh_screenshot_for_url(state, target_url, "on-demand-fallback").await
        }
    }
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

struct FetchedPreviewMetadata {
    resolved_url: Url,
    metadata: ExtractedMetadata,
}

async fn fetch_preview_payload(target_url: Url, state: &AppState) -> Result<PreviewPayload, &'static str> {
    let fetched = fetch_preview_metadata(target_url, &state.config).await?;
    let screenshot_image = if fetched.metadata.image.is_none() {
        resolve_screenshot_image_for_preview(state, &fetched.resolved_url).await
    } else {
        None
    };

    Ok(PreviewPayload {
        ok: true,
        url: Some(fetched.resolved_url.to_string()),
        title: fetched.metadata.title,
        description: fetched.metadata.description,
        image: fetched.metadata.image.or(screenshot_image),
        error: None,
    })
}

async fn fetch_preview_metadata(
    target_url: Url,
    config: &PreviewRuntimeConfig,
) -> Result<FetchedPreviewMetadata, &'static str> {
    let mut current_url = target_url;

    for hop in 0..=config.max_redirects {
        let response = send_pinned_request(&current_url, config).await?;

        if response.status().is_redirection() {
            if hop == config.max_redirects {
                return Err("too many redirects");
            }

            let next_url = parse_and_validate_redirect_target(&current_url, &response).await?;
            current_url = next_url;
            continue;
        }

        if !response.status().is_success() {
            return Err("received non-success response");
        }

        let body = read_limited_body(response, config.response_max_bytes).await?;
        return Ok(FetchedPreviewMetadata {
            resolved_url: current_url.clone(),
            metadata: extract_metadata(&body, &current_url),
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

async fn send_pinned_request(
    target_url: &Url,
    config: &PreviewRuntimeConfig,
) -> Result<reqwest::Response, &'static str> {
    ensure_url_shape_is_allowed(target_url)?;

    let host = target_url.host_str().ok_or("URL host is required")?;
    let host_port = target_url.port_or_known_default().ok_or("URL port is required")?;

    if host.parse::<IpAddr>().is_ok() {
        let client = build_preview_client(None, config)?;
        return client
            .get(target_url.clone())
            .send()
            .await
            .map_err(|_| "failed to fetch URL");
    }

    let resolved_ips = resolve_and_validate_host(host, host_port, config).await?;

    for pinned_ip in resolved_ips.into_iter().take(config.max_resolved_ip_attempts) {
        let pinned_socket = SocketAddr::new(pinned_ip, host_port);
        let client = build_preview_client(Some((host, pinned_socket)), config)?;

        match client.get(target_url.clone()).send().await {
            Ok(response) => return Ok(response),
            Err(_) => continue,
        }
    }

    Err("failed to fetch URL")
}

async fn fetch_screenshot_image(target_url: &Url, config: &PreviewRuntimeConfig) -> Option<String> {
    let worker_base_url = config.screenshot_worker_url.as_ref()?;
    let capture_url = worker_base_url.join("capture").ok()?;
    let client = reqwest::Client::builder()
        .timeout(config.screenshot_worker_timeout)
        .connect_timeout(config.connect_timeout)
        .redirect(Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .ok()?;

    let mut request = client.get(capture_url).query(&[("url", target_url.as_str())]);
    if let Some(token) = config.screenshot_worker_token.as_ref() {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let payload = response
        .json::<ScreenshotWorkerCaptureResponse>()
        .await
        .ok()?;
    if !payload.ok {
        return None;
    }

    payload
        .image
        .or(payload.image_data_url)
        .and_then(normalize_text)
}

fn build_preview_client(
    resolved_host: Option<(&str, SocketAddr)>,
    config: &PreviewRuntimeConfig,
) -> Result<reqwest::Client, &'static str> {
    let mut client_builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(config.request_timeout)
        .connect_timeout(config.connect_timeout)
        .user_agent(USER_AGENT);

    if let Some((host, pinned_socket)) = resolved_host {
        client_builder = client_builder.resolve(host, pinned_socket);
    }

    client_builder
        .build()
        .map_err(|_| "failed to prepare request client")
}

async fn resolve_and_validate_host(
    host: &str,
    port: u16,
    config: &PreviewRuntimeConfig,
) -> Result<Vec<IpAddr>, &'static str> {
    let resolved_hosts = timeout(config.dns_lookup_timeout, lookup_host((host, port)))
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
            screenshot_cache: Arc::new(RwLock::new(ScreenshotCacheStore {
                file_path: PathBuf::from("/tmp/test-preview-cache.json"),
                entries: HashMap::new(),
            })),
            screenshot_refresh_in_flight: Arc::new(RwLock::new(HashSet::new())),
            config: PreviewRuntimeConfig {
                cache_ttl_seconds: DEFAULT_PREVIEW_CACHE_TTL_SECONDS,
                cache_max_entries: DEFAULT_PREVIEW_CACHE_MAX_ENTRIES,
                response_max_bytes: DEFAULT_PREVIEW_RESPONSE_MAX_BYTES,
                request_timeout: Duration::from_millis(DEFAULT_PREVIEW_REQUEST_TIMEOUT_MS),
                connect_timeout: Duration::from_millis(DEFAULT_PREVIEW_CONNECT_TIMEOUT_MS),
                dns_lookup_timeout: Duration::from_millis(DEFAULT_PREVIEW_DNS_LOOKUP_TIMEOUT_MS),
                max_redirects: DEFAULT_PREVIEW_MAX_REDIRECTS,
                max_resolved_ip_attempts: DEFAULT_PREVIEW_MAX_RESOLVED_IP_ATTEMPTS,
                screenshot_worker_url: None,
                screenshot_worker_timeout: Duration::from_millis(
                    DEFAULT_SCREENSHOT_WORKER_TIMEOUT_MS,
                ),
                screenshot_worker_token: None,
                screenshot_ttl_seconds: DEFAULT_SCREENSHOT_TTL_SECONDS,
                screenshot_stale_grace_seconds: DEFAULT_SCREENSHOT_STALE_GRACE_SECONDS,
                screenshot_cache_index_path: PathBuf::from("/tmp/test-preview-cache.json"),
                screenshot_refresh_token: Some("token".to_string()),
                screenshot_refresh_concurrency: DEFAULT_SCREENSHOT_REFRESH_CONCURRENCY,
                screenshot_refresh_urls_path: PathBuf::from("config/preview-urls.json"),
            },
        };
        let now = Instant::now();

        {
            let mut cache = state.cache.write().await;

            for index in 0..DEFAULT_PREVIEW_CACHE_MAX_ENTRIES {
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
        assert_eq!(cache.len(), DEFAULT_PREVIEW_CACHE_MAX_ENTRIES);
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

    #[test]
    fn screenshot_cache_decision_reports_fresh() {
        let now: u64 = 1_700_000_000;
        let entry = ScreenshotCacheEntry {
            image: "data:image/png;base64,fresh".to_string(),
            captured_at: now.saturating_sub(10),
            expires_at: now.saturating_add(20),
            source: "scheduled-refresh".to_string(),
            last_error: None,
        };

        let decision = decide_screenshot_cache_action(now, Some(&entry), 120);
        assert_eq!(decision, ScreenshotCacheDecision::Fresh);
    }

    #[test]
    fn screenshot_cache_decision_reports_stale_within_grace() {
        let now: u64 = 1_700_000_000;
        let entry = ScreenshotCacheEntry {
            image: "data:image/png;base64,stale".to_string(),
            captured_at: now.saturating_sub(500),
            expires_at: now.saturating_sub(5),
            source: "scheduled-refresh".to_string(),
            last_error: None,
        };

        let decision = decide_screenshot_cache_action(now, Some(&entry), 60);
        assert_eq!(decision, ScreenshotCacheDecision::StaleWithinGrace);
    }

    #[test]
    fn screenshot_cache_decision_reports_missing_or_expired() {
        let now: u64 = 1_700_000_000;
        let entry = ScreenshotCacheEntry {
            image: "data:image/png;base64,expired".to_string(),
            captured_at: now.saturating_sub(500),
            expires_at: now.saturating_sub(120),
            source: "scheduled-refresh".to_string(),
            last_error: None,
        };

        assert_eq!(
            decide_screenshot_cache_action(now, Some(&entry), 60),
            ScreenshotCacheDecision::MissingOrExpired
        );
        assert_eq!(
            decide_screenshot_cache_action(now, None, 60),
            ScreenshotCacheDecision::MissingOrExpired
        );
    }
}

async fn read_limited_body(
    response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<String, &'static str> {
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::with_capacity(8192);

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|_| "failed reading response body")?;

        if body.len() + chunk.len() > max_response_bytes {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotWorkerCaptureResponse {
    ok: bool,
    image: Option<String>,
    #[serde(alias = "imageDataUrl")]
    image_data_url: Option<String>,
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
