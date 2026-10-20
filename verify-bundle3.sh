#!/bin/bash
echo "=== source: where is the label ==="
grep -rlF "切换到浅色模式" /tmp/lux-u11/web/src | head
echo "=== source: LuxShell has toggleTheme ==="
grep -cF "toggleTheme" /tmp/lux-u11/web/src/components/layout/LuxShell.tsx
echo "=== built: label occurrences per file ==="
docker exec lux-testlib sh -c "grep -cF 切换到浅色模式 /usr/local/share/lux/web/assets/index-CU_itJCS.js" 2>/dev/null
docker exec lux-testlib sh -c "ls /usr/local/share/lux/web/assets/ | head -5"
echo "=== built: aria-label 切换到深色模式 ==="
docker exec lux-testlib sh -c "grep -lF 切换到深色模式 /usr/local/share/lux/web/assets/*.js"
echo "=== css silver rule ==="
docker exec lux-testlib sh -c "grep -oF 'data-lux-accent=silver]' /usr/local/share/lux/web/assets/index-CU_itJCS.css | head -3"