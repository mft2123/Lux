#!/bin/bash
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d '{"username":"admin","password":"Test@12345a"}' -o /dev/null
echo "=== libraries ==="
curl -s -b $J "http://127.0.0.1:8099/api/v1/libraries" | head -c 400; echo
LIB=$(curl -s -b $J "http://127.0.0.1:8099/api/v1/libraries" | python3 -c "import sys,json; d=json.load(sys.stdin); items=d.get('libraries') or d.get('items') or []; print(items[0]['id'] if items else '')")
echo "lib=$LIB"
echo "=== first items (poster tags) ==="
curl -s -b $J "http://127.0.0.1:8099/api/v1/items?parentId=$LIB&limit=3" \
 | python3 -c "
import sys,json
d=json.load(sys.stdin)
items=d.get('items') or d.get('Items') or []
for it in items[:3]:
    print(it.get('id'), it.get('name'), '| type=',it.get('itemType'), '| tags=', it.get('imageTags'))
"