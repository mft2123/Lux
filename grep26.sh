#!/bin/bash
cd /tmp/lux-u11
echo "=== client construction ==="
grep -n "TelegramClient(\|async def check_session" -A 22 sidecar/telegram/tgwatch/runner.py | head -50