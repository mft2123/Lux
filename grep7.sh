#!/bin/bash
cd /tmp/lux-u11
echo "=== AdminSettingsPatch ==="
grep -rn "struct AdminSettingsPatch" -A 25 src/ | head -35
echo "=== update handler ==="
grep -rn "fn admin_update_settings\|update_admin_settings" src/api --include=*.rs | head -5