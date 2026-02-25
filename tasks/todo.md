- [x] Restate goal + acceptance criteria
- [x] Locate existing implementation / patterns
- [x] Design: minimal Yew + Trunk approach
- [x] Implement Yew app and remove conflicting static assets
- [x] Run verification (`cargo check`, `trunk build --release`)
- [x] Summarize changes + verification story

## Acceptance Criteria
- Replace static HTML/CSS/JS setup with a working Rust + Yew + Trunk homepage app.
- Preserve content intent and section order for Kyler Cao portfolio.
- Include theme toggle in Yew with safe localStorage persistence and system preference fallback.
- Keep accessibility requirements: landmarks/headings, focus-visible, persistent link affordance, reduced motion, and accessible new-tab indication.
- `cargo check` and `trunk build --release` complete successfully.

## Working Notes
- Existing site already has the required content order and visual direction; migrate content and style into Yew.
- Keep dependencies minimal: `yew`, `web-sys`.

## Results
- Replaced static page bootstrapping with a Yew CSR app mounted at `#app`.
- Implemented theme toggle and persistence in Rust with resilient localStorage + media-query fallback.
- Verified `cargo check` and production `trunk build --release` after installing missing toolchain pieces (`trunk`, `wasm32-unknown-unknown`).

## 2026-02-24 nd.mt refinement
- [x] Restate goal + acceptance criteria
- [x] Read Yew component and stylesheet structure
- [x] Refine layout/typography to single-column minimal flow
- [x] Fix link-at-rest accessibility issue without adding visual chrome
- [x] Keep theme toggle a11y state and reduced-motion behavior
- [x] Run verification (`cargo check`, `trunk build --release`)
- [x] Summarize changes + verification story

### Acceptance Criteria
- Preserve Kyler content while aligning to nd.mt-like minimal layout.
- Remove borders/cards/shadows and keep single 640px centered column.
- Use required typography and spacing values.
- Ensure links are identifiable at rest without relying on color alone.
- Keep existing theme tokens and toggle accessibility behavior.
- Maintain secure new-tab semantics for external links.

### Results
- Tightened layout and typography to a plain single-column rhythm with 640px max width and no section chrome.
- Converted project/link rows to compact inline "link - descriptor" style and reused secure external-link component for consistent new-tab indication.
- Fixed link affordance at rest using persistent underlines, preserving minimalist aesthetics.
- Added non-intrusive bootstrap warning in `index.html` catch block.
- Verified with `cargo check` and `trunk build --release`.

## 2026-02-24 nd.mt animation parity
- [x] Restate goal + acceptance criteria
- [x] Read existing theme toggle and link styles
- [x] Add View Transitions-based theme swap with fallback
- [x] Match nd.mt link underline behavior (hover/focus only)
- [x] Add root view-transition curtain animations and reduced-motion handling
- [x] Run verification (`cargo check`, `trunk build --release`)
- [x] Summarize changes + verification story

### Acceptance Criteria
- Theme toggles with `document.startViewTransition` when available and falls back cleanly.
- Theme state persistence and button a11y labels/pressed state remain intact.
- Base links are not underlined at rest, with hover/focus `.link::after` animation.
- Root view transition uses ~300ms curtain/slide effect and obeys reduced motion.

### Results
- Added dynamic View Transitions API interop in Yew to wrap `data-theme` attribute mutation.
- Kept theme persistence plus `aria-label`/`aria-pressed` updates unchanged.
- Replaced static link underlines with nd.mt-like `.link::after` hover/focus animation.
- Added root view-transition keyframes and explicit reduced-motion disablement.

## 2026-02-24 trunk serve rebuild loop
- [x] Restate goal + acceptance criteria
- [x] Reproduce loop with `trunk serve` and capture trigger evidence
- [x] Identify changing file(s) and causal watch path
- [x] Apply minimal config-level fix
- [x] Verify `trunk serve` stability after fix
- [x] Summarize root cause, file changes, and exact commands

### Acceptance Criteria
- Reproduce the repeated rebuild behavior locally with observable logs.
- Pinpoint the specific file path(s) changing and explain why they change.
- Apply minimal fix that preserves behavior while preventing watch-loop rebuilds.
- Confirm rebuilds do not repeat without source edits after the fix.

### Results
- Observed Trunk writing generated artifacts under `dist/` and `target/` during build (e.g., `dist/index.html`, `dist/.stage/*`, `target/wasm-bindgen/debug/*`).
- Trace logs showed these are build outputs and should not trigger source rebuilds; to harden against watch-loop behavior, added explicit watch ignores.
- Added minimal `Trunk.toml` with `[watch].ignore = ["dist", "target"]` so generated files are never treated as watch inputs.
- Re-ran `trunk serve` and confirmed a single initial build with no repeated rebuilds during idle period.

## 2026-02-24 backend preview API + frontend integration
- [x] Restate goal + acceptance criteria
- [x] Restructure crate setup so frontend and backend build cleanly
- [x] Implement backend static serving + `/api/preview` with validation, SSRF guards, limits, and TTL cache
- [x] Integrate Yew hover/focus preview fetching with graceful fallback to local assets
- [x] Run verification (`cargo check`, `trunk build --release`, `cargo build --release`, API sanity test)
- [ ] Commit and push to `https://github.com/kyler505/portfolio.git`

### Acceptance Criteria
- Backend serves built frontend from `dist/` and handles `GET /api/preview?url=...`.
- Preview API enforces http/https parsing, SSRF protections, timeout/body/redirect limits, metadata extraction, compact JSON, and cache headers.
- Frontend keeps current behavior/style while enriching hover cards with API metadata and local fallback assets.
- Accessibility and reduced-motion behavior remain intact.

### Results
- Added a native Axum backend path (same crate, cfg-based) that serves `dist/`, exposes `/api/preview`, validates + resolves URLs, blocks private/local addresses, limits redirects/body/timeouts, extracts OG/Twitter metadata, and caches responses in-memory with TTL.
- Preserved Yew interaction patterns while enriching hover previews with async metadata hydration from `/api/preview` and local asset fallback when API data is missing or fails.
- Kept reduced-motion and keyboard focus behavior intact while extending hover cards to display image/title/description.
- Verified with `cargo check`, `trunk build --release`, `cargo build --release`, and a live `curl` sanity call against the running backend.
- Required verification commands run with outcomes captured.

## 2026-02-25 self-hosted screenshot fallback
- [x] Restate goal + acceptance criteria
- [x] Add self-hosted screenshot worker (`/capture`, `/health`) with SSRF-safe URL validation
- [x] Integrate backend screenshot fallback after OG/Twitter extraction with env-based runtime knobs
- [x] Fix frontend hydration fallback so loading copy does not stick after failed metadata fetch
- [x] Update Render blueprint and README for two-service deployment and local dev
- [x] Run verification (`cargo check`, `cargo test backend::tests`, `trunk build --release`, `node --check screenshot-worker/server.js`)
- [ ] Commit and push to `origin/main`

### Acceptance Criteria
- Introduce self-hosted screenshot worker with URL validation and Playwright capture.
- Keep preview API contract stable while adding backend screenshot fallback when metadata image is absent.
- Preserve existing backend SSRF protections and degrade gracefully when worker is unavailable.
- Ensure frontend preview card leaves loading state after hydration failure.
- Document and configure two-service Render deployment.

## 2026-02-25 hybrid screenshot strategy (scheduled + on-demand)
- [x] Restate goal + acceptance criteria
- [x] Read existing backend preview/screenshot and Render wiring
- [x] Add persistent screenshot cache index with TTL + stale grace env controls
- [x] Update `/api/preview` hybrid behavior (fresh/stale/missing branches)
- [x] Add token-protected `POST /internal/refresh-screenshots` with bounded concurrency
- [x] Add refresh URL config + Render cron caller integration
- [x] Update README docs for behavior/env/config/cron
- [x] Run verification (`cargo check`, `cargo test backend::tests`, `trunk build --release`, `node --check screenshot-worker/server.js`)
- [ ] Commit and push to `origin/main`

### Acceptance Criteria
- Prefer cached screenshots refreshed by schedule.
- Keep on-demand screenshot fallback for missing or too-old cache entries.
- Persist screenshot metadata index to writable temp path and mirror in memory.
- Provide authenticated internal batch refresh endpoint using shared safety checks.
- Configure Render cron service for daily refresh calls.

### Working Notes
- Keep frontend behavior unchanged except existing loading/fallback semantics.
- Keep existing `/api/preview` request hardening and limits unchanged.

### Results
- Added a persistent screenshot cache index (disk + in-memory mirror) with fresh/stale/missing decision branches for `/api/preview` fallback behavior.
- Added authenticated `POST /internal/refresh-screenshots` batch refresh using shared URL safety checks and bounded concurrency.
- Added `config/preview-urls.json`, a cron caller script, Render cron wiring, and README updates for hybrid behavior + env configuration.

## 2026-02-25 screenshot fallback structured logging hardening
- [x] Restate goal + acceptance criteria
- [x] Read backend preview/refresh and worker capture pipelines
- [x] Add structured backend logs with request-id propagation and safe URL logging controls
- [x] Add structured worker logs with request lifecycle, validation, and Playwright stage events
- [x] Update README logging/debug guidance
- [x] Run verification (`cargo check`, `cargo test backend::tests`, `trunk build --release`, `node --check screenshot-worker/server.js`)
- [ ] Commit and push to `origin/main`

### Acceptance Criteria
- `/api/preview` logs request lifecycle, cache decisions, OG fetch outcome, screenshot fallback outcome, and response timings.
- `/internal/refresh-screenshots` logs auth/config failures and completion summary counts.
- Backend and worker correlate logs via `x-request-id`; backend forwards request id to worker.
- Logging defaults avoid sensitive URL query output, with env-tunable URL log mode.
- Worker logs capture lifecycle, validation reasons, route abort reasons, and Playwright stage events without token leakage.

## 2026-02-25 production screenshot fallback manual debug + fix
- [x] Restate goal + acceptance criteria
- [x] Reproduce failing `/api/preview` request against production and capture `x-request-id`
- [x] Correlate backend + worker Render logs and isolate exact failing stage
- [x] Apply minimal safe code/config fix for root cause
- [x] Run verification (`cargo check`, `cargo test backend::tests`, `trunk build --release`, `node --check screenshot-worker/server.js`)
- [x] Validate runtime success path for previously failing URL and confirm structured logs
- [ ] Commit and push to `origin/main`

### Acceptance Criteria
- Identify the precise production failure stage with request-id-correlated evidence.
- Apply the smallest change that restores screenshot fallback while preserving SSRF and token protections.
- Produce successful `/api/preview` response with screenshot fallback for a URL lacking OG image.
- Verification commands pass locally and logs clearly show fallback success path.

### Results
- Reproduced failures with request IDs showing backend `screenshot_worker_failed` (`worker_failure_reason":"upstream"`) and no worker `/capture` logs while `SCREENSHOT_WORKER_URL` used private host.
- Updated Render config/docs to use public service endpoints and set `PLAYWRIGHT_BROWSERS_PATH=0` so Chromium is packaged in the worker deploy artifact.
- Verified production success using `req-1771990512967-7`: worker logs show `capture_goto_start`, `capture_goto_ok`, `capture_screenshot_ok`, and backend logs show `preview_screenshot_fallback` with `worker_succeeded:true`.
