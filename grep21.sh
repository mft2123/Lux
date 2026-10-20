#!/bin/bash
cd /tmp/lux-u11
sed -n '30,58p' sidecar/telegram/main.py
echo "---- run cmd ----"
grep -n "_cmd_run" -A 30 sidecar/telegram/main.py | head -45