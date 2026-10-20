#!/bin/bash
cd /tmp/lux-u11
echo "=== main.py size + commands ==="
wc -l sidecar/telegram/main.py
grep -n "add_parser\|def cmd_\|def main" sidecar/telegram/main.py | head -20
echo "=== compose fragment ==="
cat sidecar/telegram/compose.fragment.yml