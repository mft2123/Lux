#!/bin/bash
echo "--- sidecar deep health (good cookie) ---"
curl -s -m 25 "http://127.0.0.1:8098/healthz?deep=1"; echo
echo "--- containers ---"
docker ps --filter name=lux- --format "{{.Names}} {{.Status}}"
echo "--- image ---"
docker images --format "{{.Repository}}:{{.Tag}} {{.ID}}" | grep -E "lux:phase-a|lux-transfer115"
echo "--- settings ---"
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings \
  | python3 -c "import sys,json; d=json.load(sys.stdin); t=d['transfer115']; print({'cookie':t['cookie'],'sidecar_base':t['sidecar_base'],'target_dir':t['target_dir']})"