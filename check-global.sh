#!/bin/bash
echo "=== new chunk strings ==="
CHUNK=$(docker exec lux-testlib sh -c "grep -l 保存媒体库显示 /usr/local/share/lux/web/assets/*.js" | head -1 | sed "s|.*/||")
echo "chunk=$CHUNK"
echo "测试连接 occurrences: $(docker exec lux-testlib sh -c "grep -o 测试连接 /usr/local/share/lux/web/assets/$CHUNK | wc -l")"
echo "保存媒体库显示 occurrences: $(docker exec lux-testlib sh -c "grep -o 保存媒体库显示 /usr/local/share/lux/web/assets/$CHUNK | wc -l")"

J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)

echo "=== baseline admin settings ==="
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print({'resumeMinTicks':d.get('resumeMinTicks'),'forceAdminLibraryOrder':d.get('forceAdminLibraryOrder'),'showMetadataPending':(d.get('mediaStrategy') or {}).get('showMetadataPending'),'loginBackgroundSource':d.get('loginBackgroundSource')})"

echo "=== save playback only (15 min) ==="
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"resumeMinTicks":9000000000}' -o /dev/null -w "PUT %{http_code}\n"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print({'resumeMinTicks':d.get('resumeMinTicks'),'forceAdminLibraryOrder':d.get('forceAdminLibraryOrder'),'showMetadataPending':(d.get('mediaStrategy') or {}).get('showMetadataPending')})"

echo "=== save library display only ==="
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"forceAdminLibraryOrder":true}' -o /dev/null -w "PUT %{http_code}\n"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print({'resumeMinTicks':d.get('resumeMinTicks'),'forceAdminLibraryOrder':d.get('forceAdminLibraryOrder')})"