#!/bin/bash
cd /tmp/lux-u11
echo "=== sidecar call in archive.rs ==="
grep -n "transfer115_base\|transfer_url\|healthz\|/transfer" src/application/archive.rs | head -20
echo "=== reqwest client construction ==="
grep -n "reqwest::Client\|Client::builder" src/application/archive.rs | head -10