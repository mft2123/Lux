#!/bin/bash
echo "=== source has the block? ==="
grep -c "浅色主题可读性" /tmp/lux-u11/web/src/react.css
echo "=== built css: any light selectors for admin? ==="
docker exec lux-testlib sh -c "grep -o 'data-lux-theme=light][^{]*lux-admin-panel-heading[^{]*' /usr/local/share/lux/web/assets/*.css" | head -3
echo "=== count light selectors in built css ==="
docker exec lux-testlib sh -c "grep -o 'data-lux-theme=light]' /usr/local/share/lux/web/assets/*.css" | wc -l
echo "=== settings css file ==="
docker exec lux-testlib sh -c "ls /usr/local/share/lux/web/assets/*.css" | head