#!/bin/bash
set -e

PHOSEUM_SETUP_BIN="$HOME/.cargo/bin/phoseum-setup"
SECRET_STORE="$HOME/.phoseum-googleapis-secret.json"

basedir="$(cd $(dirname $0); pwd)"
. "$basedir/config"

if [ -e "$SECRET_STORE" ]; then
    echo "Secret store already exists: $SECRET_STORE. Remove it first to continue" >&2
    exit 1
fi

exec $PHOSEUM_SETUP_BIN "$GOOGLE_OAUTH_CLIENT_ID" "$GOOGLE_OAUTH_CLIENT_SECRET"
