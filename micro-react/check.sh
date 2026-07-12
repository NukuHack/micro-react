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

echo "==> Done"
