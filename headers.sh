#!/bin/bash
echo "=== index.html cache headers ==="
curl -s -I http://127.0.0.1:8099/ | head -15
echo "=== asset cache headers ==="
ASSET=$(curl -s http://127.0.0.1:8099/ | grep -o 'assets/index-[A-Za-z0-9_.-]*\.js' | head -1)
echo "asset=$ASSET"
curl -s -I "http://127.0.0.1:8099/$ASSET" | head -8