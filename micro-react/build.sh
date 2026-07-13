#!/usr/bin/env bash
# build.sh — Test then build micro-react-wasm.
set -euo pipefail

SKIP_TESTS=false
RUN_TESTS=false

# Parse flags
while [[ $# -gt 0 ]]; do
  case "$1" in
    -y) RUN_TESTS=true; shift ;;
    -n) SKIP_TESTS=true; shift ;;
    *) echo "Usage: $0 [-y | -n]"; echo "  -y  Run all tests then build"; echo "  -n  Skip tests, compile only"; exit 1 ;;
  esac
done

if ! $RUN_TESTS && ! $SKIP_TESTS; then
  echo "Usage: $0 [-y | -n]"
  echo "  -y  Run all tests then build"
  echo "  -n  Skip tests, compile only"
  exit 1
fi

echo "==> Checking prerequisites..."
command -v rustup    >/dev/null || { echo "ERROR: rustup not found. Install from https://rustup.rs"; exit 1; }
command -v wasm-pack >/dev/null || { echo "INFO: Installing wasm-pack..."; cargo install wasm-pack; }

echo "==> Adding wasm32 target..."
rustup target add wasm32-unknown-unknown

if $RUN_TESTS; then
  echo "==> Running pure-logic tests (cargo test --lib)..."
  set +e
  cargo test --lib
  LIB_TEST_STATUS=$?
  set -e
  if [ "$LIB_TEST_STATUS" -ne 0 ]; then
    echo "==> WARNING: pure-logic tests failed (exit $LIB_TEST_STATUS). Continuing anyway."
  fi

  echo "==> Running DOM-backed tests (wasm-pack test --headless --firefox)..."
  set +e
  wasm-pack test --headless --firefox
  WASM_TEST_STATUS=$?
  set -e
  if [ "$WASM_TEST_STATUS" -ne 0 ]; then
    echo "==> WARNING: DOM-backed tests failed or the browser driver could not run (exit $WASM_TEST_STATUS)."
    echo "    Continuing to build anyway."
  fi
else
  echo "==> Skipping tests (-n flag)."
fi

echo "==> Building release..."
wasm-pack build --target web --release

echo "==> Build complete. Output in pkg/"