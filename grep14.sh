#!/bin/bash
cd /tmp/lux-u11
echo "=== theme switching code ==="
grep -rn "data-lux-theme\|lux-theme\|setTheme\|themeMode" web/src --include=*.tsx --include=*.ts | grep -v test | head -20
echo "=== telegram sidecar http? ==="
ls sidecar/telegram
grep -rn "def do_GET\|HTTPServer\|healthz\|/status" sidecar/telegram/*.py | head -20