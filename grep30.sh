#!/bin/bash
cd /tmp/lux-u11
echo "=== cover/artwork modules in Lux ==="
grep -rln "poster\|fanart\|cover" src/application --include=*.rs | head -12
echo "=== web cover components ==="
ls web/src/features/media 2>/dev/null | head -20
echo "=== daquan cover files ==="
docker run --rm chendaquan/daquanclaw:latest sh -c "ls /app | grep -iE 'cover|art|poster|image'" 2>/dev/null | head -12