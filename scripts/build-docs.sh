#!/bin/sh
# Build the technical reference manual into build/docs/.
# Same script runs locally (via `make docs`) and in CI. Fails fast with a
# clear message if Asciidoctor is missing; never installs anything itself.
set -eu

if ! command -v asciidoctor >/dev/null 2>&1; then
  echo "error: 'asciidoctor' not found on PATH." >&2
  echo "Install the minimum dependency first:" >&2
  echo "  macOS:  brew install asciidoctor" >&2
  echo "  Debian/Ubuntu:  sudo apt install asciidoctor" >&2
  echo "  Fedora:  sudo dnf install asciidoctor" >&2
  echo "  RubyGems (any OS):  gem install asciidoctor rouge" >&2
  exit 1
fi

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
SRC="$ROOT/docs/index.adoc"
OUT="$ROOT/build/docs"

if [ ! -f "$SRC" ]; then
  echo "error: entry point not found: $SRC" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT"

# Document attributes (toc, sectnums, docinfo, ...) live in docs/index.adoc;
# this stays a plain build so local and CI output cannot drift apart.
asciidoctor -D "$OUT" "$SRC"

# Theme assets referenced by docs/theme/docinfo.html (relative "theme/...").
mkdir -p "$OUT/theme"
cp "$ROOT/docs/theme/docs.css" "$ROOT/docs/theme/nav.js" "$OUT/theme/"

# Figures referenced via :imagesdir: (relative "images/...").
if [ -n "$(ls -A "$ROOT/docs/images" 2>/dev/null)" ]; then
  mkdir -p "$OUT/images"
  cp "$ROOT/docs/images/"* "$OUT/images/"
fi

echo "docs built: $OUT/index.html"
