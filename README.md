# My Personal Portfolio

This repo is a Rust portfolio app with:
- a Yew frontend (built by Trunk), and
- an integrated Axum backend in `src/backend.rs` that serves `dist/` and exposes `GET /api/preview`.
- a self-hosted screenshot worker in `screenshot-worker/` (Node + Playwright) used as an image fallback.

The backend now uses a hybrid screenshot strategy:
- scheduled screenshot refreshes populate a persistent cache index, and
- `/api/preview` can still capture on-demand when cache data is missing or too old.

## Architecture

Render blueprint uses two services:
1. `portfolio` (Rust web service): serves frontend assets and handles `/api/preview`.
2. `screenshot-worker` (Node web service): handles `GET /capture?url=...` + unauthenticated `GET/HEAD /`, `/health`, and `/uptime`, and returns data-URL screenshots.

Preview flow:
- Rust backend fetches OG/Twitter metadata first.
- If metadata already has an image, that image is returned.
- If metadata has no image, backend applies screenshot cache semantics:
  - fresh (`now < expires_at`): return cached screenshot,
  - stale in grace (`expires_at <= now <= expires_at + grace`): return stale screenshot immediately and trigger async refresh,
  - missing/expired beyond grace: call screenshot worker on-demand; cache on success, otherwise return metadata without image.
- Screenshot metadata is persisted to `SCREENSHOT_CACHE_INDEX_PATH` (default `/tmp/preview-cache.json`) and mirrored in memory.

Internal refresh endpoint:
- `POST /internal/refresh-screenshots`
- token-protected via `SCREENSHOT_REFRESH_TOKEN` (`Authorization: Bearer <token>`)
- reads URLs from `config/preview-urls.json` (or `SCREENSHOT_URLS_CONFIG_PATH`)
- refreshes screenshots with bounded concurrency (`SCREENSHOT_REFRESH_CONCURRENCY`, default `3`).

## Local development

1. Install required tooling:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
```

2. Frontend-only dev loop:

```bash
trunk serve
```

This is best for UI iteration. `trunk serve` does not run the integrated backend, so `/api/preview` calls will not be served there.

3. Run the integrated server (frontend + API):

```bash
trunk build --release
cargo run --release
```

Build order matters: the backend serves static assets from `dist/`, so build the frontend first, then start the Rust server.

4. Run screenshot worker locally (second terminal):

```bash
npm install --prefix screenshot-worker
npx --prefix screenshot-worker playwright install chromium
node screenshot-worker/server.js
```

5. Enable screenshot fallback in backend (optional):

```bash
SCREENSHOT_WORKER_URL=http://127.0.0.1:3001 cargo run --release
```

6. Trigger a manual refresh batch locally (optional):

```bash
SCREENSHOT_REFRESH_TOKEN=dev-token \
  curl -X POST http://127.0.0.1:8080/internal/refresh-screenshots \
  -H "Authorization: Bearer dev-token"
```

## Preview API security notes

`/api/preview` in `src/backend.rs` includes high-level SSRF protections:
- allows only `http`/`https` URLs,
- blocks localhost, private/loopback/link-local/multicast/documentation IP ranges,
- re-validates redirect targets,
- pins outbound requests to validated DNS results,
- applies request, connect, DNS, redirect, and response-size limits.

`/capture` in `screenshot-worker/server.js` mirrors SSRF-safe URL checks:
- only allows `http`/`https`,
- blocks localhost and private/link-local/loopback/multicast/documentation targets,
- applies explicit DNS lookup timeouts during hostname validation,
- validates every Playwright network request with `page.route("**/*")` before it is allowed,
- enforces main-frame document navigation to remain on the original hostname (redirects to other hosts are blocked),
- aborts requests fail-closed when URL validation is uncertain or invalid.

## Verification commands

```bash
cargo check
trunk build --release
cargo test backend::tests
node --check screenshot-worker/server.js
```

## UptimeRobot monitor target

Use an HTTP(s) monitor pointed at the screenshot worker's public URL. `/uptime` is still the recommended path; `/` now also returns 200 for GET/HEAD probes.

Exact path to monitor:
- `https://<your-screenshot-worker-host>/uptime`

Alternative (supported):
- `https://<your-screenshot-worker-host>/`

Examples:
- Render public service URL: `https://<render-generated-hostname>/uptime`
- Local sanity check: `http://127.0.0.1:3001/uptime`

## Deploying to Render

This repo includes `render.yaml` for a two-service deployment.

- `portfolio` build command installs wasm tooling, builds the frontend, then builds the backend binary.
- `portfolio` start command runs `./target/release/portfolio`.
- `screenshot-worker` build command installs Node deps and Playwright Chromium (without `--with-deps`, because Render build containers cannot run the privileged OS package install path and it can fail with `su: Authentication failure`).
- `screenshot-worker` start command runs `node screenshot-worker/server.js`.
- If runtime logs show missing Chromium shared libraries, install the required system packages in the service environment/base image and redeploy.

Environment variables:
- `PORT` is provided by Render and used by `src/backend.rs`.
- `RUST_LOG` is included for runtime log level control.
- `SCREENSHOT_WORKER_URL` points backend fallback calls to `http://screenshot-worker:10000` in Render.
- `SCREENSHOT_WORKER_TIMEOUT_MS` controls screenshot worker request timeout.
- `SCREENSHOT_WORKER_TOKEN` is required in the blueprint for both services (configured with `sync: false` placeholders); backend sends `Authorization: Bearer <token>` and the worker rejects missing/invalid tokens.
- `SCREENSHOT_TTL_SECONDS` sets screenshot freshness TTL (default `604800`, 7 days).
- `SCREENSHOT_STALE_GRACE_SECONDS` sets stale grace window (default `1209600`, 14 days).
- `SCREENSHOT_CACHE_INDEX_PATH` sets cache index path (default `/tmp/preview-cache.json`).
- `SCREENSHOT_REFRESH_TOKEN` protects `POST /internal/refresh-screenshots` and is also used by Render cron service.
- `SCREENSHOT_URLS_CONFIG_PATH` sets the URL list config path (default `config/preview-urls.json`).
- `SCREENSHOT_REFRESH_CONCURRENCY` bounds refresh fan-out (default `3`, allowed `2-4`).
- `DNS_LOOKUP_TIMEOUT_MS` controls worker DNS resolution timeout for SSRF host validation.
- `PREVIEW_*` values are included as deploy-time placeholders for preview API tuning defaults.
  - Current runtime defaults are defined in `src/backend.rs` constants.

Maintaining refresh URLs:
- edit `config/preview-urls.json`
- supported formats:
  - object: `{ "urls": ["https://...", "https://..."] }`
  - array: `["https://...", "https://..."]`
- each URL is validated with the same SSRF-safe parser used by `/api/preview`; invalid entries are skipped and counted in refresh summary.

Render cron notes:
- `render.yaml` includes `preview-screenshot-refresh` cron scheduled daily (`0 3 * * *`).
- cron runs `node scripts/refresh-screenshots.mjs`, which calls backend `POST /internal/refresh-screenshots` with bearer token.
- `SCREENSHOT_REFRESH_ENDPOINT` defaults to `http://portfolio:10000/internal/refresh-screenshots`; update it if your Render network topology differs.

Deploy flow:
1. Push repo to GitHub.
2. In Render, create a Blueprint deployment from the repo.
3. Render reads `render.yaml`, builds, and starts the service.
