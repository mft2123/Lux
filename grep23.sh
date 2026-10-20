#!/bin/bash
cd /tmp/lux-u11
echo "=== light theme override count ==="
grep -c "data-lux-theme=\"light\"" web/src/react.css
echo "=== does light theme cover inputs / admin panels? ==="
grep -n "data-lux-theme=\"light\"" web/src/react.css | grep -E "input|textarea|select|admin-panel|admin-settings|label" | head -15
echo "=== hardcoded white-ish text (risk in light theme) ==="
grep -n "color: #fff\|color: #f5f5f5\|color: #fff;" web/src/react.css | head -10