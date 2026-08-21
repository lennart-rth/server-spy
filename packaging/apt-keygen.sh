#!/bin/sh
# Generates a signing key for the apt repository and prints the steps to
# activate it. Run once locally, then configure the secret on GitHub.
set -e

KEY_NAME="server-spy apt repo"
KEY_EMAIL="lennart-rth@users.noreply.github.com"

gpg --batch --gen-key 2>&1 <<EOF
%no-protection
Key-Type: eddsa
Key-Curve: ed25519
Key-Usage: sign
Name-Real: $KEY_NAME
Name-Email: $KEY_EMAIL
Expire-Date: 0
EOF

KEYID=$(gpg --list-keys --with-colons "$KEY_EMAIL" | awk -F: '/^fpr:/ {print $10; exit}')

echo
echo "key created: $KEYID"
echo
echo "next steps:"
echo "  1. export the PRIVATE key and add it as the GitHub secret APT_GPG_KEY:"
echo "     gpg --armor --export-secret-keys $KEYID | xclip -selection clipboard"
echo "     (repo -> Settings -> Secrets and variables -> Actions -> New repository secret)"
echo
echo "  no gpg on this machine? let the workflow do it:"
echo "     run the 'pages' workflow (Actions -> pages -> Run workflow) once,"
echo "     download the 'apt-gpg-key-fresh' artifact from the 'assemble apt"
echo "     repository' job and paste its contents into the APT_GPG_KEY secret."
echo
echo "  2. enable GitHub Pages once:"
echo "     repo -> Settings -> Pages -> Source: Deploy from a branch -> branch 'gh-pages'"
echo "  3. publish the repo once: Actions -> 'pages' -> Run workflow"
echo "  4. every future release rebuilds the repo automatically"
