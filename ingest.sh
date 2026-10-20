#!/bin/bash
J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- ingest ---"
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/ingest \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"url":"https://115cdn.com/s/sws1yeb3wt5?password=x3c1%EF%BC%8C%E8%AF%B7%E5%8F%8A%E6%97%B6%E6%A3%80%E6%9F%A5%E7%BD%91%E7%9B%98%E7%8A%B6%E5%86%B5%E6%88%96%E8%80%85cookie%E6%98%AF%E5%90%A6%E9%A3%8E%E6%8E%A7%E6%88%96%E8%80%85%E6%9C%89%E6%95%88%EF%BC%81","title":"Lux 115 验收测试","mode":"手动","source":"acceptance"}' \
  -w "\nHTTP %{http_code}\n"
echo "--- queue dir ---"
ls -1 /vol5/1000/build/archive-test-config/../ 2>/dev/null | head -3