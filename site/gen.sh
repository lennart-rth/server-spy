#!/bin/sh
# Generates the machine-readable metadata for the GitHub Pages site:
#   llms.txt, llms-full.txt, robots.txt, sitemap.xml, og-image.png
# and rewrites the JSON-LD / OpenGraph block in index.html.
# Run from the repo root:  site/gen.sh
set -e

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
SITE_URL="https://lennart-rth.github.io/server-spy"
RAW="https://raw.githubusercontent.com/lennart-rth/server-spy/master"
DESC="A terminal tool that measures CPU, memory and I/O congestion on shared servers and shows how much each of your experiment runs was slowed down by it."

cd site

# --- llms.txt ---
cat > llms.txt <<EOF
# server-spy

> $DESC

Key notes:

- runs as a small background daemon; the TUI attaches, detaches and reattaches at any time
- classifies your experiment processes with a simple name or regex filter and tracks each distinct parameter combination as a run
- records scheduler wait and PSI stall penalties per run, and identifies which other users and processes caused the congestion
- installs on any Linux (static binary, apt repo, cargo, nix); snapshots save to plain CSV

## Documentation

- [README]($RAW/README.md): overview, install instructions, TUI reference and keys
- [Landing page]($SITE_URL/index.md): marketing summary of features
- [Install script]($RAW/install.sh): universal no-sudo installer

## Optional

- [GitHub repository](https://github.com/lennart-rth/server-spy)
- [GitHub releases](https://github.com/lennart-rth/server-spy/releases): prebuilt binaries, .deb and .rpm packages
- [crates.io](https://crates.io/crates/server-spy)
- [Demo recording]($SITE_URL/demo.cast): the TUI in action
EOF

# --- llms-full.txt ---
cat ../README.md > llms-full.txt

# --- robots.txt ---
cat > robots.txt <<EOF
User-agent: *
Allow: /

Sitemap: $SITE_URL/sitemap.xml
EOF

# --- sitemap.xml ---
cat > sitemap.xml <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>$SITE_URL/</loc>
  </url>
</urlset>
EOF

# --- og-image.png (regenerated only if rsvg-convert is available) ---
if command -v rsvg-convert >/dev/null 2>&1; then
    cat > /tmp/server-spy-og.svg <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630" viewBox="0 0 1200 630">
  <defs>
    <radialGradient id="glow" cx="50%" cy="0%" r="90%">
      <stop offset="0%" stop-color="#00d9ff" stop-opacity="0.12"/>
      <stop offset="100%" stop-color="#00d9ff" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="1200" height="630" fill="#0b0e14"/>
  <rect width="1200" height="630" fill="url(#glow)"/>
  <text x="80" y="210" font-family="monospace" font-size="76" fill="#00d9ff" font-weight="bold">server-spy</text>
  <text x="80" y="285" font-family="sans-serif" font-size="32" fill="#c9d1d9">who slowed down your experiment on a shared server</text>
  <path d="M720 420 H860 L890 330 L960 490 L1010 360 L1030 420 H1100" fill="none" stroke="#3ddc77" stroke-width="10" stroke-linecap="round" stroke-linejoin="round"/>
  <circle cx="1100" cy="420" r="9" fill="#3ddc77"/>
  <text x="80" y="480" font-family="monospace" font-size="26" fill="#7d8590">$ curl -fsSL https://lennart-rth.github.io/server-spy/install.sh | sh</text>
  <text x="80" y="560" font-family="monospace" font-size="22" fill="#7d8590">Linux · daemon + TUI · PSI · per-run attribution · plain CSV</text>
</svg>
EOF
    rsvg-convert -w 1200 -h 630 /tmp/server-spy-og.svg -o og-image.png
fi

# --- index.md (static twin of the landing page; must exist) ---
[ -f index.md ] || { echo "site/index.md is missing" >&2; exit 1; }

# --- index.html: rewrite the machine-meta block ---
python3 - "$VERSION" "$DESC" "$SITE_URL" <<'PYEOF'
import sys
version, desc, site = sys.argv[1], sys.argv[2], sys.argv[3]

block = f"""<!-- machine-meta:start -->
<meta property="og:title" content="server-spy — who slowed down your experiment on a shared server">
<meta property="og:description" content="{desc}">
<meta property="og:type" content="website">
<meta property="og:url" content="{site}/">
<meta property="og:image" content="{site}/og-image.png">
<link rel="alternate" type="text/markdown" href="index.md">
<link rel="describedby" href="llms.txt">
<script type="application/ld+json">
{{
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "name": "server-spy",
  "description": "{desc}",
  "url": "{site}/",
  "codeRepository": "https://github.com/lennart-rth/server-spy",
  "license": "https://opensource.org/licenses/MIT",
  "version": "{version}",
  "softwareVersion": "{version}",
  "operatingSystem": "Linux",
  "applicationCategory": "DeveloperApplication",
  "installUrl": "{site}/install.sh",
  "downloadUrl": "https://github.com/lennart-rth/server-spy/releases/latest",
  "releaseNotes": "https://github.com/lennart-rth/server-spy/releases",
  "author": {{ "@type": "Person", "name": "lennart-rth", "url": "https://github.com/lennart-rth" }},
  "datePublished": "2026-08-21",
  "offers": {{ "@type": "Offer", "price": "0", "priceCurrency": "EUR" }},
  "keywords": ["monitoring", "hpc", "psi", "reproducibility", "shared-server", "experiment", "congestion", "tui", "linux"]
}}
</script>
<!-- machine-meta:end -->"""

path = "index.html"
html = open(path).read()
start = "<!-- machine-meta:start -->"
end = "<!-- machine-meta:end -->"
if start in html and end in html:
    i = html.index(start)
    j = html.index(end) + len(end)
    html = html[:i] + block + html[j:]
else:
    print("warning: machine-meta markers not found in index.html; not modified", file=sys.stderr)
open(path, "w").write(html)
print(f"site metadata generated (version {version})")
PYEOF
