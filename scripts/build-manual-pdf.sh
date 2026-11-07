#!/usr/bin/env bash
# Build docs/PicSim_Manual.pdf from docs/MANUAL.md (needs pandoc + weasyprint).
# The hand-written "# Contents" list is dropped; pandoc generates the index with page numbers.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
docs="$here/../docs"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$here/../Cargo.toml" | head -1)
python3 - "$docs/MANUAL.md" "$tmp/m.md" <<'P'
import sys
s = open(sys.argv[1]).read()
a = s.index('# Contents'); b = s.index('\n---\n', a)
s = s[:a] + s[b + 5:]
head, rest = s.split('\n', 1)          # first line is the manual title; the cover shows it
open(sys.argv[2], 'w').write('# About this manual\n' + rest)
P
cp "$docs/manual.css" "$tmp/"
(cd "$tmp" && pandoc m.md -f gfm -t html5 -s --toc --toc-depth=2 \
  --metadata title="pic-sim Manual" \
  --metadata subtitle="pic-sim and basil-sim ${version} · PIC18 and PIC16 simulator" \
  --metadata date="$(date +%F)" -c manual.css -o m.html)
weasyprint "$tmp/m.html" "$docs/PicSim_Manual.pdf"
echo "wrote docs/PicSim_Manual.pdf"
