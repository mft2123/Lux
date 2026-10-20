#!/bin/bash
KEY=$(docker inspect lux-telegram-test --format "{{range .Config.Env}}{{println .}}{{end}}" | sed -n "s/^LUX_TG_API_KEY=//p")
[ -n "$KEY" ] && echo "key len=${#KEY}" || { echo "no key"; exit 1; }
BASE=/vol5/1000/build/lux-tg-test
docker rm -f lux-telegram-test >/dev/null 2>&1 || true
docker run -d --name lux-telegram-test -p 8091:8091 --user 0 \
  -e LUX_ARCHIVE_SETTINGS=/config/archive-settings.json \
  -e LUX_TG_SESSION_DIR=/app/session -e LUX_TG_STATE_DIR=/app/state \
  -e LUX_TG_LUX_BASE=http://172.17.0.1:8099 \
  -e LUX_TG_HEALTH_PORT=8091 -e LUX_TG_LOG_LEVEL=INFO \
  -e LUX_TG_API_KEY="$KEY" \
  -v /vol5/1000/build/archive-test-config:/config:ro \
  -v $BASE/session:/app/session -v $BASE/state:/app/state \
  lux-telegram:test >/dev/null
sleep 8
echo "--- status ---"; docker ps --filter name=lux-telegram-test --format "{{.Status}}"
echo "--- healthz ---"; curl -s -m 10 http://127.0.0.1:8091/healthz || true; echo
echo "--- deep ---";     curl -s -m 40 "http://127.0.0.1:8091/healthz?deep=1" || true; echo
echo "--- channels ---"; curl -s -m 70 http://127.0.0.1:8091/channels | head -c 1200 || true; echo
echo "--- logs ---";     docker logs lux-telegram-test 2>&1 | tail -6