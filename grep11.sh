#!/bin/bash
cd /tmp/lux-u11
echo "=== secondary button exists? ==="
grep -rn "\.lux-button-secondary" web/src --include=*.css | head -3
echo "=== primary/secondary base ==="
grep -rn "\.lux-button\b" web/src --include=*.css | head -5