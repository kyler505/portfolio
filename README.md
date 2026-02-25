# My Personal Portfolio

This repo is a frontend-only Rust/Yew portfolio built with Trunk.

Hover previews are fully static:
- `TechHub` and `LinkedIn` use manual screenshots from `previews/manual/`.
- Other external links use local placeholder previews.
- No backend preview API, worker service, or runtime preview fetch exists.

## Local development

1. Install required tooling:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
```

2. Run the local dev server:

```bash
trunk serve
```

3. Build production assets:

```bash
trunk build --release
```

## Manual preview screenshots

Manual screenshots live in `previews/manual/` and are copied by Trunk through `index.html`.

When updating them, keep names stable:
- `previews/manual/techhub.png`
- `previews/manual/linkedin.png`

## Verification

```bash
cargo check
trunk build --release
```

## Deploying to Render

This repo includes `render.yaml` for a single static site deployment.

Deploy flow:
1. Push repo to GitHub.
2. In Render, create a Blueprint deployment from the repo.
3. Render reads `render.yaml`, builds with Trunk, and publishes `dist/`.
