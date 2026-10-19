#!/usr/bin/env sh
# Generates deploy/.env with random secrets for a new Sivryn instance.
set -eu

cd "$(dirname "$0")"

if [ -f .env ]; then
    echo ".env already exists; refusing to overwrite it." >&2
    exit 1
fi

printf "Domain of your instance (e.g. chat.example.com): "
read -r domain
if [ -z "$domain" ]; then
    echo "A domain is required." >&2
    exit 1
fi

rand() { openssl rand -hex "$1"; }

sed \
    -e "s|^SIVRYN_DOMAIN=.*|SIVRYN_DOMAIN=$domain|" \
    -e "s|^POSTGRES_PASSWORD=.*|POSTGRES_PASSWORD=$(rand 24)|" \
    -e "s|^LIVEKIT_API_KEY=.*|LIVEKIT_API_KEY=sivryn-$(rand 4)|" \
    -e "s|^LIVEKIT_API_SECRET=.*|LIVEKIT_API_SECRET=$(rand 32)|" \
    .env.example > .env
chmod 600 .env

cat <<EOF
Wrote deploy/.env for $domain.

Next steps:
  1. Point DNS for $domain to this server.
  2. Open ports 80/tcp, 443/tcp+udp, 7881/tcp and 7882/udp.
  3. docker compose up -d
  4. Open the desktop app, add https://$domain and register: the first
     account becomes the instance admin.
EOF
