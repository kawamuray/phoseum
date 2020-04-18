#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"

. "$basedir/config"

sudo systemctl stop phoseum
sudo -u pi /usr/bin/xset -display "$DISPLAY" dpms force off
