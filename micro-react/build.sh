#!/usr/bin/env bash
# build.sh — Build micro-react-wasm.
set -euo pipefail

PROFILE="${1:-release}"

echo "==> Checking prerequisites..."
command -v rustup    >/dev/null || { echo "ERROR: rustup not found. Install from https://rustup.rs"; exit 1; }
command -v wasm-pack  >/dev/null || { echo "INFO: Installing wasm-pack..."; cargo install wasm-pack; }

echo "==> Adding wasm32 target..."
rustup target add wasm32-unknown-unknown

echo "==> Building ($PROFILE)..."
if [ "$PROFILE" = "dev" ]; then
  wasm-pack build --target web --dev
else
  wasm-pack build --target web --release
fi

echo "==> Build complete. Output in pkg/"
echo "    micro_react_bg.wasm"
echo "    micro_react.js"
echo "    micro_react.d.ts"
echo ""
echo "==> To use in your project:"
echo "    import initWasm, * as MicroReact from './pkg/micro_react.js';"
echo "    await initWasm();"
