#!/bin/bash
cd /tmp/lux-u11
echo "=== AccountPage theme block ==="
sed -n '300,360p' web/src/features/account/AccountPage.tsx
echo "=== theme state/apply ==="
grep -n "theme" web/src/features/account/AccountPage.tsx | head -30
echo "=== tg sidecar files ==="
ls sidecar/telegram/tgwatch sidecar/telegram/tools 2>/dev/null | head -20
grep -rn "http\|serve\|port" sidecar/telegram/main.py | head -10