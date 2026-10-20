#!/bin/bash
IMG=chendaquan/daquanclaw:latest
echo "=== channelFromDialog ==="
docker run --rm $IMG sh -c "grep -n 'function channelFromDialog' -A 30 /app/telegram-account.js | head -40"
echo "=== normalizeTelegramChannelModes ==="
docker run --rm $IMG sh -c "grep -n 'function normalizeTelegramChannelModes' -A 25 /app/telegram-account.js | head -30"