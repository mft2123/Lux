#!/bin/bash
cd /tmp/lux-u11
echo "=== main.py check/deep ==="
sed -n '60,140p' sidecar/telegram/main.py
echo "=== runner deep check ==="
grep -rn "def deep_check\|async def check\|is_user_authorized\|get_me" sidecar/telegram | head -10