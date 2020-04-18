#!/bin/bash
set -e

basedir="$(cd $(dirname $0); pwd)"
. "$basedir/config"

start_bin="$basedir/phoseum-start.sh"
playlist_cmd="$basedir/playlist-cmd.sh"
sleep_bin="$basedir/phoseum-sleep.sh"
wakeup_bin="$basedir/phoseum-wakeup.sh"

sudo tee /etc/systemd/system/phoseum.service <<EOF >/dev/null
[Unit]
Description=Photo and video slideshow
Requires=network-online.target phoseum-pl-refresh.timer phoseum-pl-update.timer
After=network-online.target lightdm.service

[Service]
User=pi
Group=pi
ExecStart=$start_bin
Restart=always
RestartSec=30
KillMode=process

[Install]
WantedBy=graphical.target
EOF

# Playlist refresh
sudo tee /etc/systemd/system/phoseum-pl-refresh.service <<EOF >/dev/null
[Unit]
Description=Refresh phoseum playlist
BindsTo=phoseum.service
After=phoseum.service

[Service]
Type=oneshot
ExecStart=$playlist_cmd refresh
EOF
sudo tee /etc/systemd/system/phoseum-pl-refresh.timer <<EOF >/dev/null
[Unit]
Description=Refresh phoseum playlist
BindsTo=phoseum.service

[Timer]
OnCalendar=$REFRESH_CAL

[Install]
WantedBy=timers.target
EOF

# Playlist update
sudo tee /etc/systemd/system/phoseum-pl-update.service <<EOF >/dev/null
[Unit]
Description=Update phoseum playlist
BindsTo=phoseum.service
After=phoseum.service

[Service]
Type=oneshot
ExecStart=$playlist_cmd update
EOF
sudo tee /etc/systemd/system/phoseum-pl-update.timer <<EOF >/dev/null
[Unit]
Description=Update phoseum playlist
BindsTo=phoseum.service

[Timer]
OnCalendar=$UPDATE_CAL

[Install]
WantedBy=timers.target
EOF

# Sleep
sudo tee /etc/systemd/system/phoseum-sleep.service <<EOF >/dev/null
[Unit]
Description=Enter sleep mode

[Service]
Type=oneshot
ExecStart=$sleep_bin
EOF
sudo tee /etc/systemd/system/phoseum-sleep.timer <<EOF >/dev/null
[Unit]
Description=Enter sleep mode

[Timer]
OnCalendar=$SLEEP_CAL

[Install]
WantedBy=timers.target
EOF

# Wake up
sudo tee /etc/systemd/system/phoseum-wakeup.service <<EOF >/dev/null
[Unit]
Description=Back from sleep mode

[Service]
Type=oneshot
ExecStart=$wakeup_bin
EOF
sudo tee /etc/systemd/system/phoseum-wakeup.timer <<EOF >/dev/null
[Unit]
Description=Back from sleep mode

[Timer]
OnCalendar=$WAKEUP_CAL

[Install]
WantedBy=timers.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable phoseum

sudo systemctl enable phoseum-pl-refresh.timer
sudo systemctl enable phoseum-pl-update.timer
sudo systemctl enable phoseum-sleep.timer
sudo systemctl enable phoseum-wakeup.timer
sudo systemctl start phoseum-pl-refresh.timer
sudo systemctl start phoseum-pl-update.timer
sudo systemctl start phoseum-sleep.timer
sudo systemctl start phoseum-wakeup.timer
