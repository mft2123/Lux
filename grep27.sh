#!/bin/bash
cd /tmp/lux-u11
echo "=== option / select rules ==="
grep -nE "option|select" web/src/react.css | grep -viE "selection|selector" | head -20
echo "=== custom select menu colors ==="
grep -n "lux-select-menu\|lux-select-option" web/src/react.css | head -10