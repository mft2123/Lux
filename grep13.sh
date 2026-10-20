#!/bin/bash
cd /tmp/lux-u11
echo "=== token definitions ==="
grep -n "lux-status-success\|lux-status-danger\|lux-status-warning" web/src/react.css | head -10
echo "=== existing status classes ==="
grep -n "\.lux-login-background-status\|\.lux-network-proxy-status\|\.lux-settings-saved\|\.lux-error-copy" -A 3 web/src/react.css | head -40