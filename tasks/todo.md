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
