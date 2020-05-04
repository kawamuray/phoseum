#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"

$basedir/player-cmd.sh wakeup
# Power on HDMI output
/usr/bin/tvservice --preferred
