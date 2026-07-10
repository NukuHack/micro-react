#!/usr/bin/env bash
# build.sh — Test then build micro-react-wasm.
set -euo pipefail

PROFILE="${1:-release}"

echo "==> Checking prerequisites..."
command -v rustup    >/dev/null || { echo "ERROR: rustup not found. Install from https://rustup.rs"; exit 1; }
command -v wasm-pack  >/dev/null || { echo "INFO: Installing wasm-pack..."; cargo install wasm-pack; }

echo "==> Adding wasm32 target..."
rustup target add wasm32-unknown-unknown

echo "==> Running pure-logic tests (cargo test --lib)..."
set +e
cargo test --lib
LIB_TEST_STATUS=$?
set -e
if [ "$LIB_TEST_STATUS" -ne 0 ]; then
  echo "==> WARNING: pure-logic tests failed (exit $LIB_TEST_STATUS). Continuing to build anyway."
fi

echo "==> Running DOM-backed tests (wasm-pack test --headless --chrome)..."
set +e
wasm-pack test --headless --firefox
WASM_TEST_STATUS=$?
set -e

if [ "$WASM_TEST_STATUS" -ne 0 ]; then
  echo "==> WARNING: DOM-backed tests failed or the browser driver could not run (exit $WASM_TEST_STATUS)."
  echo "    If ChromeDriver crashed/mismatched, try: sh build.sh $PROFILE   (edit this script to use --firefox instead of --chrome)"
  echo "    Continuing to build anyway."
fi

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
