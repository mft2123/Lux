#!/bin/bash
cd /tmp/lux-u11
echo "=== applyAccountTheme ==="
grep -rn "applyAccountTheme" web/src --include=*.ts --include=*.tsx | head -5
grep -rn "export function applyAccountTheme" -A 25 web/src | head -35
echo "=== accentColor usage ==="
grep -rn "accentColor\|accent-color\|data-lux-accent" web/src --include=*.ts --include=*.tsx --include=*.css | head -25