#!/bin/sh
# Build weather_shark for the web and write it into the portfolio's demo folder.
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install wasm-bindgen-cli --version <same as wasm-bindgen in Cargo.lock>
set -e
cd "$(dirname "$0")/.."
DEMO="${1:-$HOME/Projects/portfolio/demos/weather_shark}"
cargo build --release --target wasm32-unknown-unknown
mkdir -p "$DEMO/pkg"
wasm-bindgen --target web --no-typescript --out-dir "$DEMO/pkg" --out-name weather_shark \
  target/wasm32-unknown-unknown/release/weather_shark.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    "$DEMO/pkg/weather_shark_bg.wasm" -o "$DEMO/pkg/weather_shark_bg.wasm"
fi
[ -f "$DEMO/index.html" ] || cp web/index.html "$DEMO/index.html"
echo "wrote $DEMO"
