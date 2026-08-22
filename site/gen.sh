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
  <text x="80" y="155" font-family="monospace" font-size="84" fill="#00d9ff" font-weight="bold">server-spy</text>
  <text x="80" y="255" font-family="sans-serif" font-size="42" fill="#e6edf3" font-weight="800">Gain confidence about the conditions</text>
  <text x="80" y="310" font-family="sans-serif" font-size="42" fill="#e6edf3" font-weight="800">and comparability of your experiments.</text>
  <path d="M900 145 H965 L985 95 L1015 195 L1045 105 L1060 145 H1110" fill="none" stroke="#3ddc77" stroke-width="16" stroke-linecap="round" stroke-linejoin="round"/>
  <polygon points="1100,128 1135,145 1100,162" fill="#3ddc77"/>
  <g>
    <rect x="240" y="380" width="720" height="170" rx="12" fill="#0f141f" stroke="#1e2635" stroke-width="2"/>
    <rect x="240" y="380" width="720" height="36" rx="12" fill="#11151f"/>
    <rect x="240" y="404" width="720" height="12" fill="#11151f"/>
    <circle cx="266" cy="398" r="5.5" fill="#ff5f56"/>
    <circle cx="286" cy="398" r="5.5" fill="#ffbd2e"/>
    <circle cx="306" cy="398" r="5.5" fill="#27c93f"/>
    <text x="600" y="404" font-family="monospace" font-size="17" fill="#7d8590" text-anchor="middle">server-spy</text>
    <rect x="266" y="432" width="150" height="8" rx="4" fill="#2a3342"/>
    <rect x="432" y="432" width="46" height="8" rx="4" fill="#2a3342"/>
    <rect x="492" y="432" width="46" height="8" rx="4" fill="#2a3342"/>
    <rect x="552" y="432" width="46" height="8" rx="4" fill="#2a3342"/>
    <rect x="612" y="432" width="46" height="8" rx="4" fill="#2a3342"/>
    <rect x="672" y="432" width="46" height="8" rx="4" fill="#2a3342"/>
    <rect x="266" y="456" width="150" height="16" rx="8" fill="#c9d1d9"/>
    <rect x="432" y="456" width="60" height="16" rx="8" fill="#00d9ff"/>
    <rect x="500" y="456" width="30" height="16" rx="8" fill="#00d9ff" opacity="0.55"/>
    <rect x="540" y="456" width="30" height="16" rx="8" fill="#00d9ff" opacity="0.35"/>
    <rect x="580" y="456" width="34" height="16" rx="8" fill="#3ddc77"/>
    <rect x="258" y="486" width="684" height="22" rx="8" fill="#46506b"/>
    <rect x="272" y="489" width="138" height="16" rx="8" fill="#ffffff" opacity="0.92"/>
    <rect x="432" y="489" width="70" height="16" rx="8" fill="#7fdfff"/>
    <rect x="512" y="489" width="30" height="16" rx="8" fill="#7fdfff" opacity="0.6"/>
    <rect x="552" y="489" width="30" height="16" rx="8" fill="#7fdfff" opacity="0.4"/>
    <rect x="592" y="489" width="28" height="16" rx="8" fill="#3ddc77"/>
    <rect x="266" y="522" width="150" height="16" rx="8" fill="#c9d1d9" opacity="0.55"/>
    <rect x="432" y="522" width="52" height="16" rx="8" fill="#00d9ff" opacity="0.7"/>
    <rect x="492" y="522" width="30" height="16" rx="8" fill="#00d9ff" opacity="0.4"/>
    <rect x="532" y="522" width="30" height="16" rx="8" fill="#00d9ff" opacity="0.25"/>
    <rect x="572" y="522" width="30" height="16" rx="8" fill="#3ddc77" opacity="0.8"/>
  </g>
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
<meta property="og:title" content="server-spy — gain confidence about the conditions and comparability of your experiments">
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
# cache-bust the static assets: GitHub Pages/CDN caches them by filename, so
# browsers keep serving a stale style.css after every push
import re
html = re.sub(r'href="style\.css"', f'href="style.css?v={version}"', html)
html = re.sub(r'href="asciinema-player\.css"', f'href="asciinema-player.css?v={version}"', html)
html = re.sub(r'src="asciinema-player\.min\.js"', f'src="asciinema-player.min.js?v={version}"', html)
open(path, "w").write(html)
print(f"site metadata generated (version {version})")
PYEOF
