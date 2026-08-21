#!/bin/sh
# server-spy apt repository installer
# Usage: curl -fsSL https://lennart-rth.github.io/server-spy/install.sh | sudo sh
set -e

REPO_URL="https://lennart-rth.github.io/server-spy"

if [ "$(id -u)" -ne 0 ]; then
    echo "please run as root (sudo sh)" >&2
    exit 1
fi

KEY_URL="$REPO_URL/server-spy.gpg.key"
if curl -fsSL "$KEY_URL" -o /tmp/server-spy.gpg.key 2>/dev/null; then
    gpg --batch --yes --dearmor -o /usr/share/keyrings/server-spy.gpg /tmp/server-spy.gpg.key
    OPTIONS="[signed-by=/usr/share/keyrings/server-spy.gpg]"
else
    echo "unsigned repository, using trusted=yes" >&2
    OPTIONS="[trusted=yes]"
fi

echo "deb $OPTIONS $REPO_URL stable main" > /etc/apt/sources.list.d/server-spy.list
apt-get update
apt-get install -y server-spy
echo "installed: $(server-spy --version)"
