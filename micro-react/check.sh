#!/usr/bin/env sh
# Runs formatting, linting, and soft style checks (file length, line length).
# Usage: ./check.sh
set -e

MAX_LINE_WIDTH=150
MAX_FILE_LINES=500

echo "==> Running cargo fmt"
cargo fmt

echo "==> Running cargo clippy"
cargo clippy -- -D warnings

echo "==> Checking file and line length (warnings only)"

warned=0

find . -type f -name "*.rs" -not -path "./target/*" | while IFS= read -r file; do
	total_lines=$(wc -l < "$file")

	if [ "$total_lines" -gt "$MAX_FILE_LINES" ]; then
		echo "WARN: $file has $total_lines lines (limit: $MAX_FILE_LINES) — consider splitting into submodules"
	fi

	awk -v file="$file" -v max="$MAX_LINE_WIDTH" '
		{
			if (length($0) > max) {
				printf "WARN: %s:%d exceeds %d chars (%d)\n", file, NR, max, length($0)
			}
		}
	' "$file"
done

# ── Miri, best-effort (src/hooks.rs raw-pointer path) ──
#
# `hooks_get_mut`/`current_inst` in src/hooks.rs (the `unsafe fn hooks_get_mut`
# and the `unsafe { &mut (*inst)... }` derefs it and its call sites use) are
# where this crate's `unsafe` is concentrated, relying on the invariant noted
# there: "WASM is single-threaded; inst is valid for the duration of a
# render." There's no automated check that a future refactor can't
# accidentally break that (e.g. by letting a `ComponentInst`'s `Vec<HookSlot>`
# reallocate, or the instance itself move, while a stale raw pointer from an
# earlier render is still live) — Miri is the right tool to catch that class
# of bug (invalid/dangling-pointer UB), so we attempt it here.
#
# This is soft/best-effort, not a hard gate, for two reasons:
#   1. Miri requires the nightly toolchain + the `miri` component, which
#      isn't assumed to be installed in every dev environment.
#   2. Miri doesn't support the wasm32-unknown-unknown target, so this can
#      only run the plain `cargo test --lib` (non-wasm-bindgen-test) surface.
if command -v cargo-miri >/dev/null 2>&1 || cargo +nightly miri --version >/dev/null 2>&1; then
	echo "==> Running cargo miri test --lib"
	if ! cargo +nightly miri test --lib; then
		echo "WARN: cargo miri test --lib failed or found UB — investigate before trusting the unsafe hook-slot path"
	fi
else
	echo "==> Skipping Miri (nightly + 'miri' rustup component not found)"
fi

echo "==> Done"
