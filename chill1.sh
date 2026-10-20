#!/bin/bash
echo "=== /app tree (2 levels) ==="
docker exec cinechill sh -c "ls -1 /app; echo ---; ls -1 /app/core 2>/dev/null" 2>&1 | head -40
echo "=== pyc/so count ==="
docker exec cinechill sh -c "find /app -name '*.pyc' -o -name '*.so' 2>/dev/null | wc -l"
echo "=== config dir ==="
ls -la /vol5/1000/docker/chillposter/config 2>/dev/null | head -30