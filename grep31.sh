#!/bin/bash
cd /tmp/lux-u11
echo "=== where posters render ==="
grep -rln "lux-media-art\|posterUrl\|poster_url\|primaryImage" web/src --include=*.tsx | head -10
echo "=== card component ==="
grep -rn "lux-media-art" web/src --include=*.tsx | head -6