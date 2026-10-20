#!/bin/bash
cd /tmp/lux-u11
echo "=== patch type ==="
sed -n '200,275p' src/api/admin/admin_settings.rs
echo "=== update_admin_settings ==="
grep -rn "pub(crate) fn update_admin_settings" -A 40 src/application/settings* src/application/settings/*.rs 2>/dev/null | head -50