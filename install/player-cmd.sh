#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"
. "$basedir/config"

cmd="$1"
if [ -z "$cmd" ]; then
    echo "Command argument is required" >&2
    exit 1
fi

curl -v --fail -X POST "http://localhost:$CONTROL_HTTP_PORT/player/$cmd"
echo "Player command submitted: $cmd"
