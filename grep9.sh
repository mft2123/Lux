#!/bin/bash
cd /tmp/lux-u11
grep -rn "struct UpdatePlaybackSettingsRequest" -A 30 src/ | head -40
echo "=== AdminSettingsPatch TS ==="
grep -n "AdminSettingsPatch" -A 15 web/src/lib/api/types.ts | head -25