#!/bin/bash
cd /tmp/lux-u11
echo "=== AccountPage settings/effect ==="
sed -n '70,100p' web/src/features/account/AccountPage.tsx
echo "=== LuxShell theme effect ==="
sed -n '45,65p' web/src/components/layout/LuxShell.tsx
echo "=== header actions ==="
grep -n "lux-header" web/src/components/layout/LuxShell.tsx | head -10