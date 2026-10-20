#!/bin/bash
cd /tmp/lux-u11
echo "=== admin panel base ==="
grep -n "^\.lux-admin-panel" -A 2 web/src/react.css | head -12
echo "=== light theme admin overrides ==="
grep -n "data-lux-theme=\"light\".*lux-admin" web/src/react.css | head -12