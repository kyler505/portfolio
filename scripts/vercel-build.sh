#!/bin/sh
set -eu

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.93.1 --target wasm32-unknown-unknown
export PATH="$HOME/.cargo/bin:$PATH"
cargo install trunk --version 0.21.14 --locked
trunk build --release
