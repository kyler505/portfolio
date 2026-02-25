#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This project is frontend-only. Run `trunk serve` or `trunk build --release`.");
}

#[cfg(target_arch = "wasm32")]
fn main() {
    frontend::run();
}

#[cfg(target_arch = "wasm32")]
mod frontend {
    use js_sys::{ArrayBuffer, Function, WebAssembly};
    use wasm_bindgen::{closure::Closure, JsCast};
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
    const GITHUB_LINK_SCREENSHOT: &str = "/previews/manual/github.png";
    const METRIC_ROTATION_MS: i32 = 3200;
    const COMMITS_THIS_MONTH_FALLBACK: &str = "12";
    const ENERGY_START_YEAR: i32 = 2026;
    const ENERGY_START_MONTH: u32 = 1;
    const ENERGY_START_DAY: u32 = 12;

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

    #[derive(Clone, PartialEq, Eq)]
    struct Metric {
        value: AttrValue,
        label: &'static str,
    }

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct SimpleDate {
        year: i32,
        month: u32,
        day: u32,
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

    fn persist_theme(theme: Theme) {
        if let Some(storage) = local_storage() {
            let _ = storage.set_item(THEME_KEY, theme.as_str());
        }
    }

    fn js_eval_string(source: &str) -> Option<String> {
        let evaluator = Function::new_no_args(source);
        evaluator
            .call0(&wasm_bindgen::JsValue::NULL)
            .ok()?
            .as_string()
    }

    fn formatted_college_station_time() -> String {
        js_eval_string(
            "try {
                return new Intl.DateTimeFormat('en-US', {
                    timeZone: 'America/Chicago',
                    hour: 'numeric',
                    minute: '2-digit',
                    second: '2-digit',
                    hour12: true
                }).format(new Date());
            } catch (_) {
                return null;
            }",
        )
        .unwrap_or_else(|| "time unavailable".to_owned())
    }

    fn chicago_iso_date() -> Option<SimpleDate> {
        let iso = js_eval_string(
            "try {
                return new Intl.DateTimeFormat('en-CA', {
                    timeZone: 'America/Chicago',
                    year: 'numeric',
                    month: '2-digit',
                    day: '2-digit'
                }).format(new Date());
            } catch (_) {
                return null;
            }",
        )
        .or_else(|| {
            let now = js_sys::Date::new_0();
            let year = now.get_utc_full_year();
            let month = now.get_utc_month() + 1;
            let day = now.get_utc_date();
            Some(format!("{year:04}-{month:02}-{day:02}"))
        })?;

        let mut parts = iso.split('-');
        let year = parts.next()?.parse::<i32>().ok()?;
        let month = parts.next()?.parse::<u32>().ok()?;
        let day = parts.next()?.parse::<u32>().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        let max_day = days_in_month(year, month);
        if day == 0 || day > max_day {
            return None;
        }

        Some(SimpleDate { year, month, day })
    }

    fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap_year(year) => 29,
            2 => 28,
            _ => 30,
        }
    }

    fn next_day(date: SimpleDate) -> SimpleDate {
        let max_day = days_in_month(date.year, date.month);
        if date.day < max_day {
            return SimpleDate {
                day: date.day + 1,
                ..date
            };
        }

        if date.month < 12 {
            return SimpleDate {
                year: date.year,
                month: date.month + 1,
                day: 1,
            };
        }

        SimpleDate {
            year: date.year + 1,
            month: 1,
            day: 1,
        }
    }

    fn day_offset(start: SimpleDate, end: SimpleDate) -> Option<u32> {
        if end < start {
            return None;
        }

        let mut cursor = start;
        let mut days: u32 = 0;
        while cursor < end {
            cursor = next_day(cursor);
            days = days.checked_add(1)?;
        }
        Some(days)
    }

    fn weekdays_since_energy_start() -> u32 {
        let start = SimpleDate {
            year: ENERGY_START_YEAR,
            month: ENERGY_START_MONTH,
            day: ENERGY_START_DAY,
        };
        let Some(today) = chicago_iso_date() else {
            return 0;
        };
        let Some(offset) = day_offset(start, today) else {
            return 0;
        };

        let total_days = offset + 1;
        let full_weeks = total_days / 7;
        let remainder = total_days % 7;
        let mut weekdays = full_weeks * 5;
        let mut i = 0;
        while i < remainder {
            if i < 5 {
                weekdays += 1;
            }
            i += 1;
        }
        weekdays
    }

    fn format_wasm_heap_size(bytes: u64) -> String {
        const KIB: f64 = 1024.0;
        const MIB: f64 = KIB * 1024.0;

        if bytes >= (MIB as u64) {
            let value = (bytes as f64) / MIB;
            return format!("{value:.1} MB");
        }

        if bytes >= (KIB as u64) {
            let value = (bytes as f64) / KIB;
            return format!("{value:.1} KB");
        }

        format!("{bytes} B")
    }

    fn wasm_heap_size_value() -> String {
        let memory = wasm_bindgen::memory()
            .dyn_into::<WebAssembly::Memory>()
            .ok();
        let Some(memory) = memory else {
            return "heap unavailable".to_owned();
        };

        let buffer = memory.buffer().dyn_into::<ArrayBuffer>().ok();
        let Some(buffer) = buffer else {
            return "heap unavailable".to_owned();
        };

        format_wasm_heap_size(buffer.byte_length() as u64)
    }

    fn current_metrics() -> [Metric; 4] {
        [
            Metric {
                value: AttrValue::from(wasm_heap_size_value()),
                label: "Wasm heap size",
            },
            Metric {
                value: AttrValue::from(formatted_college_station_time()),
                label: "local time in College Station",
            },
            Metric {
                value: AttrValue::from(weekdays_since_energy_start().to_string()),
                label: "energy drinks consumed this year",
            },
            Metric {
                value: AttrValue::from(COMMITS_THIS_MONTH_FALLBACK),
                label: "commits this month",
            },
        ]
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

    fn clamp_preview_position(
        x: f64,
        y: f64,
        preview_width: f64,
        preview_height: f64,
    ) -> (f64, f64) {
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
                clamp_preview_position(
                    focus_x - preview_width,
                    focus_y,
                    preview_width,
                    preview_height,
                )
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
    }

    #[derive(Clone, PartialEq)]
    struct PreviewCardState {
        visible: bool,
        src: AttrValue,
        alt: AttrValue,
        x: f64,
        y: f64,
    }

    impl PreviewCardState {
        fn hidden() -> Self {
            Self {
                visible: false,
                src: AttrValue::from(PREVIEW_DEFAULT_IMAGE),
                alt: AttrValue::from(PREVIEW_DEFAULT_ALT),
                x: PREVIEW_GUTTER,
                y: PREVIEW_GUTTER,
            }
        }

        fn from_asset(asset: PreviewAsset, x: f64, y: f64) -> Self {
            Self {
                visible: true,
                src: asset.src,
                alt: asset.alt,
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
        if let Some(preview_asset) = explicit_preview {
            return Some(preview_asset);
        }

        if !is_preview_eligible_web_link(href.as_str()) {
            return None;
        }

        Some(PreviewAsset {
            src: AttrValue::from(PREVIEW_DEFAULT_IMAGE),
            alt: AttrValue::from(format!("{} preview placeholder", label)),
        })
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
        let active_metric = use_state(|| current_metrics()[0].clone());
        let metric_cursor = use_mut_ref(|| 0usize);
        let preview_card = use_state(PreviewCardState::hidden);
        let preview_anchor = use_state(|| Option::<PreviewAnchor>::None);
        let preview_card_ref = use_node_ref();
        let preview_size = use_state(|| (PREVIEW_INITIAL_WIDTH, PREVIEW_INITIAL_HEIGHT));

        {
            let theme = theme.clone();
            use_effect_with(*theme, move |current| {
                apply_theme(*current);
                || ()
            });
        }

        let on_toggle = {
            let theme = theme.clone();
            Callback::from(move |_| {
                let next = (*theme).toggled();
                persist_theme(next);
                apply_theme(next);
                theme.set(next);
            })
        };

        {
            let active_metric = active_metric.clone();
            let metric_cursor = metric_cursor.clone();
            use_effect_with((), move |_| {
                let mut interval_id = None;
                let mut callback = None;

                if let Some(win) = window() {
                    let tick = Closure::<dyn FnMut()>::new(move || {
                        let metrics = current_metrics();
                        let len = metrics.len();
                        if len == 0 {
                            return;
                        }

                        let next_index = {
                            let mut cursor = metric_cursor.borrow_mut();
                            *cursor = (*cursor + 1) % len;
                            *cursor
                        };

                        active_metric.set(metrics[next_index].clone());
                    });

                    interval_id = win
                        .set_interval_with_callback_and_timeout_and_arguments_0(
                            tick.as_ref().unchecked_ref(),
                            METRIC_ROTATION_MS,
                        )
                        .ok();
                    callback = Some(tick);
                }

                move || {
                    if let (Some(win), Some(handle)) = (window(), interval_id) {
                        win.clear_interval_with_handle(handle);
                    }
                    drop(callback);
                }
            });
        }

        let on_pointer_preview = {
            let preview_card = preview_card.clone();
            let preview_anchor = preview_anchor.clone();
            let preview_size = preview_size.clone();
            Callback::from(
                move |(asset, client_x, client_y): (PreviewAsset, i32, i32)| {
                    let anchor = PreviewAnchor::Pointer { client_x, client_y };
                    preview_anchor.set(Some(anchor));
                    let (preview_width, preview_height) = *preview_size;
                    let (x, y) =
                        preview_position_from_anchor(anchor, preview_width, preview_height);
                    preview_card.set(PreviewCardState::from_asset(asset, x, y));
                },
            )
        };

        let on_focus_preview = {
            let preview_card = preview_card.clone();
            let preview_anchor = preview_anchor.clone();
            let preview_size = preview_size.clone();
            Callback::from(move |asset: PreviewAsset| {
                let anchor = PreviewAnchor::Focus;
                preview_anchor.set(Some(anchor));
                let (preview_width, preview_height) = *preview_size;
                let (x, y) = preview_position_from_anchor(anchor, preview_width, preview_height);
                preview_card.set(PreviewCardState::from_asset(asset, x, y));
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
                ((*preview_card).visible, (*preview_card).src.clone()),
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
                                    preview={PreviewAsset {
                                        src: AttrValue::from("/previews/manual/techhub.png"),
                                        alt: AttrValue::from("TechHub website screenshot"),
                                    }}
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
                                            href="https://github.com/NujhatJalil/SHADE-project"
                                            label="Project SHADE"
                                            preview={PreviewAsset {
                                                src: AttrValue::from("/previews/og/project-shade-og.png"),
                                                alt: AttrValue::from("GitHub Open Graph image for Project SHADE repository"),
                                            }}
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — lstm team for ensemble heat-wave forecasting model"}</span>
                                    </li>
                                    <li>
                                        <ExternalLink
                                            href="https://github.com/kyler505/tamuhack25_aa"
                                            label="FlightPath"
                                            preview={PreviewAsset {
                                                src: AttrValue::from("/previews/og/flightpath-og.png"),
                                                alt: AttrValue::from("GitHub Open Graph image for FlightPath repository"),
                                            }}
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — ai flight search experience built in 2024 hours (tamuhack 25)"}</span>
                                    </li>
                                    <li>
                                        <ExternalLink
                                            href="https://github.com/kyler505/techhub-dns"
                                            label="TechHub Delivery Platform"
                                            preview={PreviewAsset {
                                                src: AttrValue::from("/previews/og/techhub-delivery-platform-og.png"),
                                                alt: AttrValue::from("GitHub Open Graph image for TechHub Delivery Platform repository"),
                                            }}
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — internal tool built from the ground up with react + flask"}</span>
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
                                            preview={PreviewAsset {
                                                src: AttrValue::from(GITHUB_LINK_SCREENSHOT),
                                                alt: AttrValue::from("Screenshot of the kyler505 GitHub profile page"),
                                            }}
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — code and experiments"}</span>
                                    </li>
                                    <li>
                                        <ExternalLink
                                            href="https://www.linkedin.com/in/kylercao"
                                            label="LinkedIn"
                                            preview={PreviewAsset {
                                                src: AttrValue::from("/previews/manual/linkedin.png"),
                                                alt: AttrValue::from("LinkedIn profile screenshot"),
                                            }}
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — professional profile"}</span>
                                    </li>
                                    <li>
                                        <ExternalLink
                                            href="/resume.pdf"
                                            label="Resume"
                                            on_pointer_preview={on_pointer_preview.clone()}
                                            on_focus_preview={on_focus_preview.clone()}
                                            on_hide_preview={on_hide_preview.clone()}
                                        />
                                        <span class="muted">{" — updated feb 5 26"}</span>
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
                            <p class="metric-value">{active_metric.value.clone()}</p>
                            <p class="metric-label">{active_metric.label}</p>
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
}
