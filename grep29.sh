#!/bin/bash
cd /tmp/lux-u11
echo "=== rail component ==="
grep -rn "HorizontalScrollRail" web/src/components/*.tsx | head -3
echo "=== rail css ==="
grep -rn "lux-home-rail\|lux-rail\|horizontal-scroll" web/src/react.css | head -12