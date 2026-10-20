#!/bin/bash
cd /tmp/lux-u11
echo "=== admin settings page files ==="
grep -rln "adminSettings\|AdminSettingsPage" web/src --include=*.tsx | head -10
echo "=== component sizes ==="
find web/src -name "*Settings*" -o -name "*settings*" | grep -v node_modules | head -20
echo "=== admin settings api ==="
grep -n "AdminSettings\b\|adminSettings()" web/src/lib/api/client.ts | head -10