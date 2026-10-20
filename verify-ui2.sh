#!/bin/bash
echo "=== css chunk has the chip rules ==="
docker exec lux-testlib sh -c "grep -l 'lux-settings-probe' /usr/local/share/lux/web/assets/*.css" || echo "MISSING"
echo "=== js chunk has both states ==="
CHUNK=$(docker exec lux-testlib sh -c "grep -l 侧车在线 /usr/local/share/lux/web/assets/*.js" | head -1 | sed 's|.*/||')
echo "chunk=$CHUNK"
for s in is-ok is-bad 正在连接侧车 测试连接 lux-button-secondary; do
  printf "%-24s %s\n" "$s" "$(docker exec lux-testlib sh -c "grep -o '$s' /usr/local/share/lux/web/assets/$CHUNK | wc -l")"
done
echo "=== served index references ==="
curl -s http://127.0.0.1:8099/ | grep -o 'assets/index-[A-Za-z0-9_.-]*\.js'