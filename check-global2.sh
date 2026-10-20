#!/bin/bash
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
show() { curl -s -b $J http://127.0.0.1:8099/api/v1/admin/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print({'resumeMinTicks':d.get('resumeMinTicks'),'forceAdminLibraryOrder':d.get('forceAdminLibraryOrder'),'showMetadataPending':(d.get('mediaStrategy') or {}).get('showMetadataPending')})"; }

echo "=== baseline ==="; show
echo "=== PATCH playback only (15 min) ==="
curl -s -b $J -X PATCH http://127.0.0.1:8099/api/v1/admin/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"resumeMinTicks":9000000000}' -o /dev/null -w "PATCH %{http_code}\n"
show
echo "=== PATCH library display only ==="
curl -s -b $J -X PATCH http://127.0.0.1:8099/api/v1/admin/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"forceAdminLibraryOrder":true}' -o /dev/null -w "PATCH %{http_code}\n"
show
echo "=== restore baseline ==="
curl -s -b $J -X PATCH http://127.0.0.1:8099/api/v1/admin/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"resumeMinTicks":1200000000,"forceAdminLibraryOrder":false}' -o /dev/null -w "PATCH %{http_code}\n"
show