#!/bin/bash
cd /tmp/lux-u11
echo "=== routes mentioning archive/settings or notify/test ==="
grep -rn "archive/settings" src/ --include=*.rs | head -10
grep -rn "notify/test" src/ --include=*.rs | head -10
echo "=== admin_jobs.rs handler names ==="
grep -n "pub async fn\|async fn" src/api/admin/admin_jobs.rs | head -40
echo "=== client.ts archive/notify methods ==="
grep -n "archiveSettings\|updateArchiveSettings\|notifyTest\|adminNotify" web/src/lib/api/client.ts | head -20