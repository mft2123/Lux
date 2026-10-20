#!/bin/bash
cd /tmp/lux-u11/sidecar/telegram && DOCKER_BUILDKIT=1 docker build -q -t lux-telegram:test . >/dev/null && echo rebuilt
KEY=$(docker inspect lux-telegram-test --format "{{range .Config.Env}}{{println .}}{{end}}" | sed -n "s/^LUX_TG_API_KEY=//p")
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
echo "--- healthz (空频道现在应为 configured) ---"; curl -s -m 10 http://127.0.0.1:8091/healthz; echo
echo "--- deep ---";  curl -s -m 40 "http://127.0.0.1:8091/healthz?deep=1"; echo
echo "--- channels count ---"; curl -s -m 70 http://127.0.0.1:8091/channels | python3 -c "import sys,json; d=json.load(sys.stdin); print('ok=',d['ok'],'count=',len(d.get('channels',[])))"