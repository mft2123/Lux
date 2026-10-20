#!/bin/bash
cd /tmp/lux-u11
echo "=== ingest client method ==="
grep -n "archiveIngest\|archiveRun" -A 6 web/src/lib/api/client.ts | head -30
echo "=== ingest types ==="
grep -n "ArchiveIngest\|LinkJob" web/src/lib/api/types.ts | head -10