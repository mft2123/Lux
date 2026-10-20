#!/bin/bash
cd /tmp/lux-u11
echo "=== 媒体库 in home ==="
grep -rn "媒体库" web/src/features/home/*.tsx 2>/dev/null | head -8
echo "=== sticky / z-index near home rows ==="
grep -rn "sticky" web/src/react.css web/src/features/home/*.css 2>/dev/null | head -12