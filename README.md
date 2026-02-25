# My Personal Portfolio

This repo is a Rust portfolio app with:
- a Yew frontend (built by Trunk), and
- an integrated Axum backend in `src/backend.rs` that serves `dist/` and exposes `GET /api/preview`.
- a self-hosted screenshot worker in `screenshot-worker/` (Node + Playwright) used as an image fallback.

## Architecture

Render blueprint uses two services:
1. `portfolio` (Rust web service): serves frontend assets and handles `/api/preview`.
2. `screenshot-worker` (Node web service): handles `GET /capture?url=...` + `/health` and returns data-URL screenshots.

Preview flow:
- Rust backend fetches OG/Twitter metadata first.
- If metadata has no usable image and `SCREENSHOT_WORKER_URL` is configured, backend calls the worker and uses the screenshot image.
- If worker capture fails, backend still returns non-image metadata without failing the full preview request.

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
- `DNS_LOOKUP_TIMEOUT_MS` controls worker DNS resolution timeout for SSRF host validation.
- `PREVIEW_*` values are included as deploy-time placeholders for preview API tuning defaults.
  - Current runtime defaults are defined in `src/backend.rs` constants.

Deploy flow:
1. Push repo to GitHub.
2. In Render, create a Blueprint deployment from the repo.
3. Render reads `render.yaml`, builds, and starts the service.
