#!/bin/bash
echo "=== policy: head ==="
docker exec daquanclaw sh -c "head -60 /app/telegram-channel-policy.js"
echo "=== policy: modes/exports ==="
docker exec daquanclaw sh -c "grep -nE 'mode|MODE|export|function ' /app/telegram-channel-policy.js | head -40"