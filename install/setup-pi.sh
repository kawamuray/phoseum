#!/bin/bash
set -e

TIMEZONE="Asia/Tokyo"

echo "Now change $(whoami) user's password"
passwd

# Persistent journald logging
sudo mkdir -p /var/log/journal
if ! grep -q "^Storage=" /etc/systemd/journald.conf >/dev/null; then
    # Creating the above with default =auto is sufficient but I don't like such ambiguousness
    echo "Storage=persistent" | sudo tee -a /etc/systemd/journald.conf >/dev/null
fi

# Package upgrades
sudo apt update
sudo apt upgrade -y

# Dependencies for compiling phoseum
sudo apt install libssl-dev
if ! [ -d $HOME/.cargo ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi

# Disable piwiz (startup wizard, doesn't disappear until we press buttons)
if [ -e /etc/xdg/autostart/piwiz.desktop ]; then
    sudo mv /etc/xdg/autostart/piwiz.desktop{,.disabled}
fi

# Disable autostarts for LXDE session
if [ -e /etc/xdg/lxsession/LXDE-pi/autostart ]; then
    sudo mv /etc/xdg/lxsession/LXDE-pi/autostart{,.disabled}
fi

# Disable daily apt updates
sudo systemctl stop apt-daily.timer
sudo systemctl stop apt-daily-upgrade.timer
sudo systemctl disable apt-daily.timer
sudo systemctl disable apt-daily-upgrade.timer

sudo timedatectl set-timezone "$TIMEZONE"
echo "Now reboot"
