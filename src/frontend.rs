use gloo_net::http::Request;
use js_sys::{encode_uri_component, Function, Reflect};
use serde::Deserialize;
use std::collections::HashMap;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{window, FocusEvent, HtmlElement, MouseEvent, Storage};
use yew::prelude::*;

const THEME_KEY: &str = "portfolio-theme";
const PREVIEW_GUTTER: f64 = 14.0;
const PREVIEW_CURSOR_OFFSET_X: f64 = 14.0;
const PREVIEW_CURSOR_OFFSET_Y: f64 = 12.0;
const PREVIEW_FOCUS_Y: f64 = 96.0;
const PREVIEW_COLUMN_WIDTH: f64 = 640.0;
const PREVIEW_INITIAL_WIDTH: f64 = 360.0;
const PREVIEW_INITIAL_HEIGHT: f64 = 260.0;
const PREVIEW_DEFAULT_IMAGE: &str = "/previews/default.svg";
const PREVIEW_DEFAULT_ALT: &str = "Project preview";
const PREVIEW_FAILURE_RETRY_MS: f64 = 10_000.0;

#[derive(Clone, Copy, PartialEq)]
enum PreviewAnchor {
    Pointer { client_x: i32, client_y: i32 },
    Focus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    fn toggle_label(self) -> String {
        let next = self.toggled().as_str();
        format!("Switch to {next} theme")
    }

    fn pressed(self) -> bool {
        matches!(self, Self::Dark)
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Light => "◐",
            Self::Dark => "◑",
        }
    }
}

fn local_storage() -> Option<Storage> {
    window()?.local_storage().ok().flatten()
}

fn read_stored_theme() -> Option<Theme> {
    let value = local_storage()?.get_item(THEME_KEY).ok().flatten()?;
    Theme::from_str(&value)
}

fn system_prefers_dark() -> bool {
    window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .map(|mq| mq.matches())
        .unwrap_or(false)
}

fn resolve_theme() -> Theme {
    read_stored_theme().unwrap_or_else(|| {
        if system_prefers_dark() {
            Theme::Dark
        } else {
            Theme::Light
        }
    })
}

fn apply_theme(theme: Theme) {
    if let Some(document) = window().and_then(|w| w.document()) {
        if let Some(root) = document.document_element() {
            let _ = root.set_attribute("data-theme", theme.as_str());
        }
    }
}

fn prefers_reduced_motion() -> bool {
    window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .map(|mq| mq.matches())
        .unwrap_or(false)
}

fn apply_theme_with_transition(theme: Theme) {
    if prefers_reduced_motion() {
        apply_theme(theme);
        return;
    }

    let Some(document) = window().and_then(|w| w.document()) else {
        apply_theme(theme);
        return;
    };

    let document_js: JsValue = document.into();
    let Ok(start_view_transition) =
        Reflect::get(&document_js, &JsValue::from_str("startViewTransition"))
    else {
        apply_theme(theme);
        return;
    };

    let Some(start_view_transition) = start_view_transition.dyn_ref::<Function>() else {
        apply_theme(theme);
        return;
    };

    let callback = Closure::<dyn FnMut()>::new(move || {
        apply_theme(theme);
    });

    if start_view_transition
        .call1(&document_js, callback.as_ref().unchecked_ref())
        .is_err()
    {
        apply_theme(theme);
    }
}

fn persist_theme(theme: Theme) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(THEME_KEY, theme.as_str());
    }
}

fn viewport_size() -> (f64, f64) {
    let Some(win) = window() else {
        return (1280.0, 720.0);
    };

    let width = win
        .inner_width()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(1280.0);
    let height = win
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(720.0);

    (width, height)
}

fn clamp_preview_position(x: f64, y: f64, preview_width: f64, preview_height: f64) -> (f64, f64) {
    let (viewport_width, viewport_height) = viewport_size();
    let min_x = PREVIEW_GUTTER;
    let min_y = PREVIEW_GUTTER;
    let max_x = (viewport_width - preview_width - PREVIEW_GUTTER).max(min_x);
    let max_y = (viewport_height - preview_height - PREVIEW_GUTTER).max(min_y);

    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

fn focus_anchor_position() -> (f64, f64) {
    let (viewport_width, _) = viewport_size();
    let column_left = ((viewport_width - PREVIEW_COLUMN_WIDTH) / 2.0).max(PREVIEW_GUTTER);
    (column_left + PREVIEW_COLUMN_WIDTH, PREVIEW_FOCUS_Y)
}

fn preview_position_from_anchor(
    anchor: PreviewAnchor,
    preview_width: f64,
    preview_height: f64,
) -> (f64, f64) {
    match anchor {
        PreviewAnchor::Pointer { client_x, client_y } => clamp_preview_position(
            f64::from(client_x) + PREVIEW_CURSOR_OFFSET_X,
            f64::from(client_y) + PREVIEW_CURSOR_OFFSET_Y,
            preview_width,
            preview_height,
        ),
        PreviewAnchor::Focus => {
            let (focus_x, focus_y) = focus_anchor_position();
            clamp_preview_position(focus_x - preview_width, focus_y, preview_width, preview_height)
        }
    }
}

fn preview_card_size(preview_card_ref: &NodeRef) -> Option<(f64, f64)> {
    let element = preview_card_ref.cast::<HtmlElement>()?;
    let width = f64::from(element.offset_width());
    let height = f64::from(element.offset_height());

    if width > 0.0 && height > 0.0 {
        Some((width, height))
    } else {
        None
    }
}

#[derive(Clone, PartialEq)]
struct PreviewAsset {
    src: AttrValue,
    alt: AttrValue,
    title: AttrValue,
    description: AttrValue,
    metadata_url: Option<AttrValue>,
}

#[derive(Clone, PartialEq)]
struct PreviewCardState {
    visible: bool,
    src: AttrValue,
    alt: AttrValue,
    title: AttrValue,
    description: AttrValue,
    metadata_url: Option<String>,
    x: f64,
    y: f64,
}

impl PreviewCardState {
    fn hidden() -> Self {
        Self {
            visible: false,
            src: AttrValue::from(PREVIEW_DEFAULT_IMAGE),
            alt: AttrValue::from(PREVIEW_DEFAULT_ALT),
            title: AttrValue::from("Preview unavailable"),
            description: AttrValue::from("Hover over a project link to view details."),
            metadata_url: None,
            x: PREVIEW_GUTTER,
            y: PREVIEW_GUTTER,
        }
    }

    fn from_asset(asset: PreviewAsset, x: f64, y: f64) -> Self {
        Self {
            visible: true,
            src: asset.src,
            alt: asset.alt,
            title: asset.title,
            description: asset.description,
            metadata_url: asset.metadata_url.map(|value| value.to_string()),
            x,
            y,
        }
    }
}

fn is_preview_eligible_web_link(href: &str) -> bool {
    let trimmed = href.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    let normalized = trimmed.to_ascii_lowercase();
    normalized.starts_with("http://") || normalized.starts_with("https://")
}

fn resolve_preview_asset(
    href: &AttrValue,
    label: &AttrValue,
    explicit_preview: Option<PreviewAsset>,
) -> Option<PreviewAsset> {
    let href_text = href.to_string();
    let eligible_web_link = is_preview_eligible_web_link(&href_text);

    if let Some(mut preview_asset) = explicit_preview {
        if preview_asset.metadata_url.is_none() && eligible_web_link {
            preview_asset.metadata_url = Some(href.clone());
        }
        return Some(preview_asset);
    }

    if !eligible_web_link {
        return None;
    }

    Some(PreviewAsset {
        src: AttrValue::from(PREVIEW_DEFAULT_IMAGE),
        alt: AttrValue::from(format!("{} preview placeholder", label)),
        title: AttrValue::from("Loading preview..."),
        description: AttrValue::from("Fetching link details..."),
        metadata_url: Some(href.clone()),
    })
}

#[derive(Clone)]
struct ApiPreviewData {
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
}

#[derive(Clone)]
enum PreviewCacheEntry {
    Ready(ApiPreviewData),
    Pending,
    Failed { retry_after_ms: f64 },
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPreviewResponse {
    ok: bool,
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
}

async fn fetch_preview(url: &str) -> Option<ApiPreviewData> {
    let encoded_url = encode_uri_component(url)
        .as_string()
        .unwrap_or_else(|| url.to_string());
    let request_url = format!("/api/preview?url={encoded_url}");

    let response = Request::get(&request_url).send().await.ok()?;
    let payload = response.json::<ApiPreviewResponse>().await.ok()?;

    if !payload.ok {
        return None;
    }

    Some(ApiPreviewData {
        title: payload.title,
        description: payload.description,
        image: payload.image,
    })
}

fn apply_remote_preview(
    preview_card: &UseStateHandle<PreviewCardState>,
    metadata_url: &str,
    remote: &ApiPreviewData,
) {
    let mut next: PreviewCardState = (**preview_card).clone();

    if !next.visible || next.metadata_url.as_deref() != Some(metadata_url) {
        return;
    }

    if let Some(title) = remote.title.as_deref() {
        next.title = AttrValue::from(title.to_string());
    }

    if let Some(description) = remote.description.as_deref() {
        next.description = AttrValue::from(description.to_string());
    }

    if let Some(image) = remote.image.as_deref() {
        next.src = AttrValue::from(image.to_string());
    }

    preview_card.set(next);
}

fn hydrate_preview(
    preview_card: &UseStateHandle<PreviewCardState>,
    preview_cache: &UseStateHandle<HashMap<String, PreviewCacheEntry>>,
    metadata_url: &str,
) {
    if let Some(cached_entry) = preview_cache.get(metadata_url) {
        match cached_entry {
            PreviewCacheEntry::Ready(cached) => {
                apply_remote_preview(preview_card, metadata_url, cached);
                return;
            }
            PreviewCacheEntry::Pending => return,
            PreviewCacheEntry::Failed { retry_after_ms } => {
                if js_sys::Date::now() < *retry_after_ms {
                    return;
                }
            }
        }
    }

    let metadata_url = metadata_url.to_string();
    let preview_card_handle = preview_card.clone();
    let preview_cache_handle = preview_cache.clone();

    {
        let mut next_cache = (**preview_cache).clone();
        next_cache.insert(metadata_url.clone(), PreviewCacheEntry::Pending);
        preview_cache.set(next_cache);
    }

    spawn_local(async move {
        let remote = fetch_preview(&metadata_url).await;

        let mut next_cache = (*preview_cache_handle).clone();
        match remote.as_ref() {
            Some(payload) => {
                next_cache.insert(
                    metadata_url.clone(),
                    PreviewCacheEntry::Ready(payload.clone()),
                );
            }
            None => {
                next_cache.insert(
                    metadata_url.clone(),
                    PreviewCacheEntry::Failed {
                        retry_after_ms: js_sys::Date::now() + PREVIEW_FAILURE_RETRY_MS,
                    },
                );
            }
        }
        preview_cache_handle.set(next_cache);

        if let Some(remote) = remote.as_ref() {
            apply_remote_preview(&preview_card_handle, &metadata_url, remote);
        }
    });
}

#[derive(Properties, PartialEq)]
struct ExternalLinkProps {
    href: AttrValue,
    label: AttrValue,
    #[prop_or_default]
    preview: Option<PreviewAsset>,
    on_pointer_preview: Callback<(PreviewAsset, i32, i32)>,
    on_focus_preview: Callback<PreviewAsset>,
    on_hide_preview: Callback<()>,
}

#[function_component(ExternalLink)]
fn external_link(props: &ExternalLinkProps) -> Html {
    let preview = resolve_preview_asset(&props.href, &props.label, props.preview.clone());

    let onmouseenter = {
        let preview = preview.clone();
        let on_pointer_preview = props.on_pointer_preview.clone();
        Callback::from(move |event: MouseEvent| {
            if let Some(preview_asset) = preview.clone() {
                on_pointer_preview.emit((preview_asset, event.client_x(), event.client_y()));
            }
        })
    };

    let onmousemove = {
        let preview = preview.clone();
        let on_pointer_preview = props.on_pointer_preview.clone();
        Callback::from(move |event: MouseEvent| {
            if let Some(preview_asset) = preview.clone() {
                on_pointer_preview.emit((preview_asset, event.client_x(), event.client_y()));
            }
        })
    };

    let onmouseleave = {
        let on_hide_preview = props.on_hide_preview.clone();
        Callback::from(move |_| on_hide_preview.emit(()))
    };

    let onfocus = {
        let preview = preview.clone();
        let on_focus_preview = props.on_focus_preview.clone();
        Callback::from(move |_event: FocusEvent| {
            if let Some(preview_asset) = preview.clone() {
                on_focus_preview.emit(preview_asset);
            }
        })
    };

    let onblur = {
        let on_hide_preview = props.on_hide_preview.clone();
        Callback::from(move |_| on_hide_preview.emit(()))
    };

    html! {
        <a
            class="link"
            href={props.href.clone()}
            target="_blank"
            rel="noopener noreferrer"
            onmouseenter={onmouseenter}
            onmousemove={onmousemove}
            onmouseleave={onmouseleave}
            onfocus={onfocus}
            onblur={onblur}
        >
            {props.label.clone()}
            <span class="sr-only">{" (opens in a new tab)"}</span>
        </a>
    }
}

#[function_component(App)]
fn app() -> Html {
    let theme = use_state(resolve_theme);
    let preview_card = use_state(PreviewCardState::hidden);
    let preview_anchor = use_state(|| Option::<PreviewAnchor>::None);
    let preview_card_ref = use_node_ref();
    let preview_size = use_state(|| (PREVIEW_INITIAL_WIDTH, PREVIEW_INITIAL_HEIGHT));
    let preview_cache = use_state(HashMap::<String, PreviewCacheEntry>::new);

    {
        let current = *theme;
        use_effect_with((), move |_| {
            apply_theme(current);
            || ()
        });
    }

    let on_toggle = {
        let theme = theme.clone();
        Callback::from(move |_| {
            let next = (*theme).toggled();
            persist_theme(next);
            apply_theme_with_transition(next);
            theme.set(next);
        })
    };

    let on_pointer_preview = {
        let preview_card = preview_card.clone();
        let preview_anchor = preview_anchor.clone();
        let preview_size = preview_size.clone();
        let preview_cache = preview_cache.clone();
        Callback::from(
            move |(asset, client_x, client_y): (PreviewAsset, i32, i32)| {
                let metadata_url = asset.metadata_url.as_ref().map(|value| value.to_string());
                let anchor = PreviewAnchor::Pointer { client_x, client_y };
                preview_anchor.set(Some(anchor));
                let (preview_width, preview_height) = *preview_size;
                let (x, y) = preview_position_from_anchor(anchor, preview_width, preview_height);
                preview_card.set(PreviewCardState::from_asset(asset, x, y));

                if let Some(metadata_url) = metadata_url {
                    hydrate_preview(&preview_card, &preview_cache, &metadata_url);
                }
            },
        )
    };

    let on_focus_preview = {
        let preview_card = preview_card.clone();
        let preview_anchor = preview_anchor.clone();
        let preview_size = preview_size.clone();
        let preview_cache = preview_cache.clone();
        Callback::from(move |asset: PreviewAsset| {
            let metadata_url = asset.metadata_url.as_ref().map(|value| value.to_string());
            let anchor = PreviewAnchor::Focus;
            preview_anchor.set(Some(anchor));
            let (preview_width, preview_height) = *preview_size;
            let (x, y) = preview_position_from_anchor(anchor, preview_width, preview_height);
            preview_card.set(PreviewCardState::from_asset(asset, x, y));

            if let Some(metadata_url) = metadata_url {
                hydrate_preview(&preview_card, &preview_cache, &metadata_url);
            }
        })
    };

    let on_hide_preview = {
        let preview_card = preview_card.clone();
        let preview_anchor = preview_anchor.clone();
        Callback::from(move |_| {
            preview_anchor.set(None);
            let mut next = (*preview_card).clone();
            next.visible = false;
            preview_card.set(next);
        })
    };

    let reclamp_preview = {
        let preview_anchor = preview_anchor.clone();
        let preview_card = preview_card.clone();
        let preview_card_ref = preview_card_ref.clone();
        let preview_size = preview_size.clone();
        Callback::from(move |_| {
            let Some(anchor) = *preview_anchor else {
                return;
            };

            let current = (*preview_card).clone();
            if !current.visible {
                return;
            }

            let measured_size = preview_card_size(&preview_card_ref).unwrap_or(*preview_size);
            if measured_size != *preview_size {
                preview_size.set(measured_size);
            }

            let (x, y) = preview_position_from_anchor(anchor, measured_size.0, measured_size.1);
            if (current.x - x).abs() < 0.1 && (current.y - y).abs() < 0.1 {
                return;
            }

            let mut next = current;
            next.x = x;
            next.y = y;
            preview_card.set(next);
        })
    };

    {
        let reclamp_preview = reclamp_preview.clone();
        let preview_card = preview_card.clone();
        use_effect_with(
            (
                (*preview_card).visible,
                (*preview_card).src.clone(),
                (*preview_card).title.clone(),
                (*preview_card).description.clone(),
            ),
            move |_| {
                reclamp_preview.emit(());
                || ()
            },
        );
    }

    {
        let reclamp_preview = reclamp_preview.clone();
        use_effect(move || {
            let win = window();
            let resize_handler = Closure::<dyn FnMut()>::new(move || {
                reclamp_preview.emit(());
            });

            if let Some(win) = win.as_ref() {
                win.set_onresize(Some(resize_handler.as_ref().unchecked_ref()));
            }

            move || {
                if let Some(win) = win {
                    win.set_onresize(None);
                }
                drop(resize_handler);
            }
        });
    }

    let on_preview_media_loaded = {
        let reclamp_preview = reclamp_preview.clone();
        Callback::from(move |_| {
            reclamp_preview.emit(());
        })
    };

    let preview_style = format!(
        "--preview-x: {:.2}px; --preview-y: {:.2}px;",
        preview_card.x, preview_card.y
    );

    html! {
        <>
            <a class="skip-link" href="#content">{"Skip to main content"}</a>
            <div class="page-shell">
                <header class="site-header" aria-labelledby="identity-heading">
                    <h1 id="identity-heading">{"Kyler Cao"}</h1>
                    <button
                        class="theme-toggle"
                        type="button"
                        aria-label={(*theme).toggle_label()}
                        aria-pressed={(*theme).pressed().to_string()}
                        onclick={on_toggle}
                    >
                        <span aria-hidden="true">{(*theme).icon()}</span>
                    </button>
                </header>

                <main id="content">
                    <section aria-labelledby="about-heading" class="section-block">
                        <h2 id="about-heading">{"About"}</h2>
                        <p>
                            {"Computer Science student at Texas A&M building dependable software for campus operations at "}
                            <ExternalLink
                                href="https://www.it.tamu.edu/services/services-by-category/desktop-and-mobile-computing/techhub.html"
                                label="TechHub"
                                on_pointer_preview={on_pointer_preview.clone()}
                                on_focus_preview={on_focus_preview.clone()}
                                on_hide_preview={on_hide_preview.clone()}
                            />
                            {" and practical machine learning projects."}
                        </p>
                    </section>

                    <section aria-labelledby="apps-heading" class="section-block">
                        <h2 id="apps-heading">{"Apps"}</h2>

                        <div class="app-group">
                            <h3>{"Builds"}</h3>
                            <ul class="row-list">
                                <li>
                                    <ExternalLink
                                        href="https://github.com/kyler505"
                                        label="Project SHADE"
                                        preview={PreviewAsset {
                                            src: AttrValue::from("/previews/shade.svg"),
                                            alt: AttrValue::from("Project SHADE sequence model dashboard preview"),
                                            title: AttrValue::from("Project SHADE"),
                                            description: AttrValue::from("LSTM component for Austin heat-wave forecasting."),
                                            metadata_url: Some(AttrValue::from("https://github.com/NujhatJalil/SHADE-project")),
                                        }}
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — LSTM component for Austin heat-wave forecasting."}</span>
                                </li>
                                <li>
                                    <ExternalLink
                                        href="https://github.com/kyler505"
                                        label="FlightPath"
                                        preview={PreviewAsset {
                                            src: AttrValue::from("/previews/flightpath.svg"),
                                            alt: AttrValue::from("FlightPath assisted trip planner interface preview"),
                                            title: AttrValue::from("FlightPath"),
                                            description: AttrValue::from("AI flight search experience from TAMUHack 2025."),
                                            metadata_url: Some(AttrValue::from("https://github.com/kyler505/tamuhack25_aa")),
                                        }}
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — AI flight search experience from TAMUHack 2025."}</span>
                                </li>
                                <li>
                                    <ExternalLink
                                        href="https://github.com/kyler505"
                                        label="TechHub Delivery Platform"
                                        preview={PreviewAsset {
                                            src: AttrValue::from("/previews/techhub.svg"),
                                            alt: AttrValue::from("TechHub delivery operations console preview"),
                                            title: AttrValue::from("TechHub Delivery Platform"),
                                            description: AttrValue::from("Internal system handling 150+ monthly orders."),
                                            metadata_url: Some(AttrValue::from("https://github.com/kyler505/techhub-dns")),
                                        }}
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — Internal system handling 150+ monthly orders."}</span>
                                </li>
                            </ul>
                        </div>

                        <div class="app-group">
                            <h3>{"Links"}</h3>
                            <ul class="row-list">
                                <li>
                                    <ExternalLink
                                        href="https://github.com/kyler505"
                                        label="GitHub"
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — Code and experiments"}</span>
                                </li>
                                <li>
                                    <ExternalLink
                                        href="https://www.linkedin.com/in/kylercao"
                                        label="LinkedIn"
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — Professional profile"}</span>
                                </li>
                                <li>
                                    <ExternalLink
                                        href="/resume.pdf"
                                        label="Resume"
                                        on_pointer_preview={on_pointer_preview.clone()}
                                        on_focus_preview={on_focus_preview.clone()}
                                        on_hide_preview={on_hide_preview.clone()}
                                    />
                                    <span class="muted">{" — Current PDF"}</span>
                                </li>
                            </ul>
                        </div>
                    </section>

                    <section aria-labelledby="languages-heading" class="section-block">
                        <h2 id="languages-heading">{"Languages"}</h2>
                        <ul class="inline-list">
                            <li><span class="muted">{"Primary"}</span>{"Java, Python, C++, JavaScript, TypeScript"}</li>
                            <li><span class="muted">{"Database"}</span>{"SQL (PostgreSQL, MySQL)"}</li>
                            <li><span class="muted">{"Also"}</span>{"C#, HTML, CSS"}</li>
                        </ul>
                    </section>

                    <section aria-labelledby="now-heading" class="section-block now-metric">
                        <h2 id="now-heading">{"Metric"}</h2>
                        <p class="metric-value">{"2027"}</p>
                        <p class="metric-label">{"expected graduation year"}</p>
                    </section>
                </main>
            </div>
            <aside
                class={classes!("hover-preview", preview_card.visible.then_some("is-visible"))}
                style={preview_style}
                aria-hidden="true"
                ref={preview_card_ref}
            >
                <img
                    class="hover-preview-media"
                    src={preview_card.src.clone()}
                    alt={preview_card.alt.clone()}
                    loading="lazy"
                    onload={on_preview_media_loaded.clone()}
                    onerror={on_preview_media_loaded}
                />
                <div class="hover-preview-copy">
                    <p class="hover-preview-title">{preview_card.title.clone()}</p>
                    <p class="hover-preview-description">{preview_card.description.clone()}</p>
                </div>
            </aside>
        </>
    }
}

pub fn run() {
    yew::Renderer::<App>::with_root(
        window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("app"))
            .expect("missing #app mount point"),
    )
    .render();
}
