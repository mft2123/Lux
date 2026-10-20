#!/bin/bash
echo "=== index chunk (header toggle) ==="
docker exec lux-testlib sh -c "grep -l 切换到浅色模式 /usr/local/share/lux/web/assets/*.js" | head -2
echo "=== settings chunk (tg probe) ==="
CHUNK=$(docker exec lux-testlib sh -c "grep -l 旁挂在线 /usr/local/share/lux/web/assets/*.js" | head -1 | sed 's|.*/||')
echo "chunk=$CHUNK"
docker exec lux-testlib sh -c "grep -o 旁挂在线 /usr/local/share/lux/web/assets/$CHUNK | wc -l"
echo "=== accent css rule ==="
CSS=$(docker exec lux-testlib sh -c "grep -l 'data-lux-accent=\"silver\"].lux-button-primary' /usr/local/share/lux/web/assets/*.css" | head -1 | sed 's|.*/||')
echo "css=$CSS"
docker exec lux-testlib sh -c "grep -o 'lux-accent-foreground);background:var(--lux-accent)' /usr/local/share/lux/web/assets/$CSS | wc -l"