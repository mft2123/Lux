#!/bin/bash
cd /tmp/lux-u11
echo "=== apply_update ==="
grep -n "fn apply_update" -A 60 src/application/archive/settings.rs | head -80