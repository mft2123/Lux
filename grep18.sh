#!/bin/bash
cd /tmp/lux-u11
echo "=== how many rules use --lux-accent ==="
grep -c "var(--lux-accent)" web/src/react.css
echo "=== primary button style ==="
grep -n "\.lux-button-primary" web/src/react.css | head -6
grep -n "\.lux-button-primary {" -A 2 web/src/react.css | head -6
echo "=== header actions markup ==="
sed -n '112,150p' web/src/components/layout/LuxShell.tsx