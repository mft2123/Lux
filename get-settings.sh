#!/bin/bash
J=/tmp/c.jar
rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "csrf_len=${#csrf}"
echo "--- settings ---"
curl -s -b $J http://127.0.0.1:8099/api/v1/admin/archive/settings | python3 -m json.tool 2>/dev/null | head -60