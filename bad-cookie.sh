#!/bin/bash
set -e
# A second sidecar with a deliberately dead cookie, so the real settings file is untouched.
docker rm -f lux-transfer115-badc >/dev/null 2>&1 || true
docker run -d --name lux-transfer115-badc -p 8097:8098 \
  -e LUX_115_COOKIE="UID=1; CID=2; SEID=3; KID=4" \
  -v /vol5/1000/build/archive-test-config:/config:ro lux-transfer115:latest >/dev/null
sleep 3
echo "--- bad sidecar deep health ---"
curl -s -m 20 "http://127.0.0.1:8097/healthz?deep=1"; echo

J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)

echo "--- point Lux at the bad sidecar ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8097"}}' -o /dev/null -w "PUT %{http_code}\n"

before=$(wc -l < "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/链接队列/状态.jsonl")
echo "state lines before: $before"

echo "--- ingest (force) + run ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/ingest \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"url":"https://115cdn.com/s/sws1yeb3wt5?password=x3c1","title":"失效 cookie 分支","mode":"手动","force":true}' -o /dev/null -w "ingest %{http_code}\n"
curl -s -m 200 -b $J -X POST "http://127.0.0.1:8099/api/v1/admin/archive/run?dry_run=false" \
  -H "X-CSRF-Token: $csrf" -o /dev/null -w "run %{http_code}\n"
sleep 2

after=$(wc -l < "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/链接队列/状态.jsonl")
echo "state lines after: $after"
echo "--- queued jobs still present? ---"
ls -1 "/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库/链接队列/"*.json 2>/dev/null | tail -3
echo "--- sidecar log ---"
docker logs lux-transfer115-badc 2>&1 | grep "transfer job" | tail -3