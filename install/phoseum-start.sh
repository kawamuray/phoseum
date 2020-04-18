#!/bin/sh
set -e

PHOSEUM_BIN=$HOME/.cargo/bin/phoseum

basedir="$(cd $(dirname $0); pwd)"
media_dir="$basedir/photos"

. "$basedir/config"

if [ ! -d "$media_dir" ]; then
    echo "Creating media dir $media_dir"
    mkdir -p "$media_dir"
fi

export DISPLAY # Value set in ./config
export RUST_BACKTRACE=full
export RUST_LOG="info,phoseum=debug"
exec $PHOSEUM_BIN \
     --googlephotos.oauth-client-id="$GOOGLE_OAUTH_CLIENT_ID" \
     --googlephotos.oauth-client-secret="$GOOGLE_OAUTH_CLIENT_SECRET" \
     --googlephotos.album-id "$GOOGLE_PHOTOS_ALBUM_ID" \
     --storage.media-dir="$media_dir" \
     --control.http-port="$CONTROL_HTTP_PORT" \
     --control.player=gpio \
     --control.gpio-dev=/dev/gpiochip0 \
     --control.gpio-map=18:H:play_next:L \
     --control.gpio-map=10:H:play_back:L \
     --control.gpio-map=23:H:pause:L \
     --control.gpio-map=23:L:resume:L \
     --control.gpio-map=24:H:mute:L \
     --control.gpio-map=24:L:unmute:L
