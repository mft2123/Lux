#!/bin/bash
cd /tmp/lux-u11
echo "=== client updateAdminSettings ==="
sed -n '1430,1445p' web/src/lib/api/client.ts
echo "=== route for /api/v1/admin/settings ==="
grep -rn "api/v1/admin/settings\"" src/api --include=*.rs | head -5