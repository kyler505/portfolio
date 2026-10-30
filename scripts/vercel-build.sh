#!/bin/sh
set -eu

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.93.1 --target wasm32-unknown-unknown
export PATH="$HOME/.cargo/bin:$PATH"
curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-musl.tar.gz -o /tmp/portfolio-trunk.tar.gz
echo 'a67f4054b249fe9acc5fabc25de1aebf19783aca3ad6ff64bf34d7da44d0ea20  /tmp/portfolio-trunk.tar.gz' | sha256sum -c -
tar -xzf /tmp/portfolio-trunk.tar.gz -C /tmp
/tmp/trunk build --release
