#!/bin/bash
cd /tmp/lux-u11
echo "=== theme color tokens ==="
grep -oE "\-\-lux-[a-z0-9-]*(success|danger|warn|error|ok|green|red)[a-z0-9-]*" web/src/react.css | sort -u | head -20
echo "=== :root palette (first 40 vars) ==="
grep -n ":root" -A 40 web/src/react.css | head -45