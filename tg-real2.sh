#!/bin/bash
docker rm -f lux-telegram-test >/dev/null 2>&1 || true
BASE=/vol5/1000/build/lux-tg-test
docker run -d --name lux-telegram-test -p 8091:8091 --user 0 \
  -e LUX_ARCHIVE_SETTINGS=/config/archive-settings.json \
  -e LUX_TG_SESSION_DIR=/app/session -e LUX_TG_STATE_DIR=/app/state \
  -e LUX_TG_LUX_BASE=http://172.17.0.1:8099 \
  -e LUX_TG_HEALTH_PORT=8091 -e LUX_TG_LOG_LEVEL=INFO \
  -v /vol5/1000/build/archive-test-config:/config:ro \
  -v $BASE/session:/app/session -v $BASE/state:/app/state \
  lux-telegram:test >/dev/null
sleep 10
docker ps --filter name=lux-telegram-test --format "{{.Status}}"
echo "--- shallow ---"; curl -s -m 10 http://127.0.0.1:8091/healthz; echo
echo "--- deep ---";    curl -s -m 45 "http://127.0.0.1:8091/healthz?deep=1"; echo
echo "--- logs ---";    docker logs lux-telegram-test 2>&1 | tail -8