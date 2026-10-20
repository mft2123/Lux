#!/bin/bash
set -e
BASE=/vol5/1000/build/lux-tg-test
mkdir -p $BASE/session $BASE/state
chmod 700 $BASE/session
# 1) 复制 chill 的 session（只读挂载源，不动原件）
docker run --rm -v /vol5/1000/docker/chillposter/config:/from:ro -v $BASE/session:/to \
  alpine sh -c "cp /from/telegram_user.session /to/lux-telegram.session && chmod 600 /to/lux-telegram.session"
ls -l $BASE/session

# 2) 把 chill 的 api_id/api_hash/phone 写进测试实例设置（不回显）
python3 - <<'PY'
import json, urllib.request, http.cookiejar
src = json.load(open("/vol5/1000/build/archive-settings.json", encoding="utf-8"))["telegram"]
jar = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
base = "http://127.0.0.1:8099"
def call(method, path, body=None, csrf=False):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("content-type", "application/json")
    if csrf:
        token = next((c.value for c in jar if c.name == "lux_csrf"), "")
        req.add_header("X-CSRF-Token", token)
    with op.open(req, timeout=20) as r:
        return r.status, r.read().decode()
call("POST", "/api/v1/auth/login", {"username": "admin", "password": "Test@12345a"})
status, _ = call("PUT", "/api/v1/admin/archive/settings", {"telegram": {
    "api_id": src["api_id"], "api_hash": src["api_hash"], "phone": src["phone"],
}}, csrf=True)
print("settings PUT", status)
PY

# 3) 重建旁挂镜像（含新的 /healthz）
cd /tmp/lux-u11/sidecar/telegram
DOCKER_BUILDKIT=1 docker build -q -t lux-telegram:test . >/dev/null 2>&1 || \
  DOCKER_BUILDKIT=1 docker build -t lux-telegram:test . 2>&1 | tail -5

# 4) 起旁挂
docker rm -f lux-telegram-test >/dev/null 2>&1 || true
docker run -d --name lux-telegram-test -p 8091:8091 \
  -e LUX_ARCHIVE_SETTINGS=/config/archive-settings.json \
  -e LUX_TG_SESSION_DIR=/app/session -e LUX_TG_STATE_DIR=/app/state \
  -e LUX_TG_LUX_BASE=http://172.17.0.1:8099 \
  -e LUX_TG_HEALTH_PORT=8091 -e LUX_TG_LOG_LEVEL=INFO \
  -v /vol5/1000/build/archive-test-config:/config:ro \
  -v $BASE/session:/app/session -v $BASE/state:/app/state \
  lux-telegram:test >/dev/null
sleep 8
docker ps --filter name=lux-telegram-test --format "{{.Status}}"
echo "--- shallow ---"; curl -s -m 10 http://127.0.0.1:8091/healthz; echo
echo "--- deep ---";    curl -s -m 40 "http://127.0.0.1:8091/healthz?deep=1"; echo
echo "--- logs ---";    docker logs lux-telegram-test 2>&1 | tail -12