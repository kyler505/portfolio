#!/bin/sh
set -eu

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.93.1 --target wasm32-unknown-unknown
export PATH="$HOME/.cargo/bin:$PATH"
curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz -o /tmp/portfolio-trunk.tar.gz
echo 'f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b  /tmp/portfolio-trunk.tar.gz' | sha256sum -c -
tar -xzf /tmp/portfolio-trunk.tar.gz -C /tmp
/tmp/trunk build --release
