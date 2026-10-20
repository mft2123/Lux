#!/bin/bash
echo "=== any chunk with the mode label besides AccountPage ==="
docker exec lux-testlib sh -c "grep -l 深浅模式 /usr/local/share/lux/web/assets/*.js" 2>/dev/null
echo "=== header button: search all js for moon/sun aria ==="
docker exec lux-testlib sh -c "grep -o '切换到[浅深]色模式' /usr/local/share/lux/web/assets/*.js" | sort | uniq -c | head
echo "=== accent rule in css ==="
docker exec lux-testlib sh -c "grep -o 'data-lux-accent=silver].lux-button-primary' /usr/local/share/lux/web/assets/*.css" | head -3
echo "=== primary button accent ==="
docker exec lux-testlib sh -c "grep -o 'background:var(--lux-accent)' /usr/local/share/lux/web/assets/*.css" | head -3