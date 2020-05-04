#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"

$basedir/player-cmd.sh sleep
# Power off HDMI output
/usr/bin/tvservice --off
