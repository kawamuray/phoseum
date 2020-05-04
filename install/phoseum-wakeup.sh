#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"

. "$basedir/config"

$basedir/player-cmd.sh wakeup
sleep 60
sudo -u pi /usr/bin/xset -display "$DISPLAY" dpms force on
