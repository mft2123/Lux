#!/bin/bash
cd /tmp/lux-u11
echo "=== client.ts imports ==="
sed -n '1,40p' web/src/lib/api/client.ts
echo "=== ArchiveTransfer115Settings ==="
grep -n "ArchiveTransfer115Settings" -A 10 web/src/lib/api/types.ts | head -20