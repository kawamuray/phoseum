#!/bin/bash
set -e

sudo systemctl stop phoseum
cargo install --git https://github.com/kawamuray/phoseum.git --force
sudo systemctl start phoseum
