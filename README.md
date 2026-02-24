# Portfolio (Yew + Rust Preview API)

This repo is a Rust portfolio app with:
- a Yew frontend (built by Trunk), and
- an integrated Axum backend in `src/backend.rs` that serves `dist/` and exposes `GET /api/preview`.

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

## Preview API security notes

`/api/preview` in `src/backend.rs` includes high-level SSRF protections:
- allows only `http`/`https` URLs,
- blocks localhost, private/loopback/link-local/multicast/documentation IP ranges,
- re-validates redirect targets,
- pins outbound requests to validated DNS results,
- applies request, connect, DNS, redirect, and response-size limits.

## Verification commands

```bash
cargo check
trunk build --release
cargo test backend::tests
```

## Deploying to Render

This repo includes `render.yaml` for a single Rust web service.

- Build command installs wasm tooling, builds the frontend, then builds the backend binary.
- Start command runs `./target/release/portfolio`.
- Health check path is `/`.

Environment variables:
- `PORT` is provided by Render and used by `src/backend.rs`.
- `RUST_LOG` is included for runtime log level control.
- `PREVIEW_*` values are included as deploy-time placeholders for preview API tuning defaults.
  - Current runtime defaults are defined in `src/backend.rs` constants.

Deploy flow:
1. Push repo to GitHub.
2. In Render, create a Blueprint deployment from the repo.
3. Render reads `render.yaml`, builds, and starts the service.
