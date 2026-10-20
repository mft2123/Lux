#!/bin/bash
set -e
echo "--- lux-testlib TMDB env ---"
docker inspect lux-testlib --format '{{range .Config.Env}}{{println .}}{{end}}' | grep -i tmdb || echo "(no TMDB env)"
echo "--- settings tmdb_api_key (masked, from API) ---"
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; print(json.load(sys.stdin).get('tmdb_api_key'))"

echo "--- restore good sidecar_base + drop bad sidecar + stale job ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8098"}}' -o /dev/null -w "PUT %{http_code}\n"
docker rm -f lux-transfer115-badc >/dev/null 2>&1 || true
rm -f "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/链接队列/1792486562551-失效-cookie-分支-v2.json"
echo "--- sidecar_base now ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['transfer115']['sidecar_base'])"
echo "--- failure dirs created by the run ---"
ls -la "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/失败目录/" | tail -4