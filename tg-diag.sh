#!/bin/bash
echo "=== container ==="
docker ps -a --filter name=lux-telegram-test --format "{{.Names}} {{.Status}} {{.Image}}"
echo "=== logs ==="
docker logs lux-telegram-test 2>&1 | tail -20
echo "=== image ==="
docker images --format "{{.Repository}}:{{.Tag}} {{.ID}}" | grep lux-telegram