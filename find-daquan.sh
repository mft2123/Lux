#!/bin/bash
echo "=== build dirs matching daquan/tg/tgzh ==="
ls -1 /vol5/1000/build 2>/dev/null | head -30
echo "=== find daquan anywhere in build/download ==="
find /vol5/1000/build /vol5/1000/download -maxdepth 3 -iname "*daquan*" 2>/dev/null | head -10
echo "=== docker images ==="
docker images --format "{{.Repository}}:{{.Tag}}" | grep -iE "daquan|tg|telegram|chill" | head -10
echo "=== containers ==="
docker ps -a --format "{{.Names}}" | head -30