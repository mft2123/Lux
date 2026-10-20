#!/bin/bash
echo "=== settings page chunk on disk ==="
docker exec lux-testlib sh -c "grep -l 侧车在线 /usr/local/share/lux/web/assets/*.js"
echo "=== does that chunk contain the button label? ==="
CHUNK=$(docker exec lux-testlib sh -c "grep -l 侧车在线 /usr/local/share/lux/web/assets/*.js" | head -1 | sed "s|.*/||")
echo "chunk=$CHUNK"
docker exec lux-testlib sh -c "grep -o 测试连接 /usr/local/share/lux/web/assets/$CHUNK | head -3"
echo "=== served html references ==="
curl -s http://127.0.0.1:8099/ | grep -o 'assets/[A-Za-z0-9_.-]*\.js' | head -5