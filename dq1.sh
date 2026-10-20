#!/bin/bash
echo "=== mounts / env ==="
docker inspect daquanclaw --format '{{range .Mounts}}{{.Source}} -> {{.Destination}} ({{.Mode}}){{println}}{{end}}'
docker inspect daquanclaw --format '{{range .Config.Env}}{{println .}}{{end}}' | grep -viE "pass|token|key|secret" | head -20
echo "=== image layout ==="
docker exec daquanclaw sh -c "ls -la / ; echo ---; ls -la /app 2>/dev/null | head -30" 2>&1 | head -45