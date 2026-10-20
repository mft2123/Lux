#!/bin/bash
IMG=chendaquan/daquanclaw:latest
echo "=== getDialogs usage ==="
docker run --rm $IMG sh -c "grep -n 'getDialogs' -B 6 -A 40 /app/telegram-account.js | head -70"