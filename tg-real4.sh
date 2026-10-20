#!/bin/bash
python3 - <<'PY'
import json, urllib.request, http.cookiejar
src = json.load(open("/vol5/1000/build/archive-settings.json", encoding="utf-8"))["telegram"]
print("template channels:", src.get("listen_channels"), "mode:", src.get("listen_mode"))
jar = http.cookiejar.CookieJar(); op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
base = "http://127.0.0.1:8099"
def call(m, p, b=None, csrf=False):
    req = urllib.request.Request(base+p, data=json.dumps(b).encode() if b is not None else None, method=m)
    req.add_header("content-type","application/json")
    if csrf: req.add_header("X-CSRF-Token", next(c.value for c in jar if c.name=="lux_csrf"))
    with op.open(req, timeout=20) as r: return r.status, r.read().decode()
call("POST","/api/v1/auth/login",{"username":"admin","password":"Test@12345a"})
st,_ = call("PUT","/api/v1/admin/archive/settings",{"telegram":{
  "listen_channels": src.get("listen_channels") or [],
  "listen_mode": src.get("listen_mode") or "全量",
}}, csrf=True)
print("settings PUT", st)
PY
docker restart lux-telegram-test >/dev/null
sleep 10
docker ps --filter name=lux-telegram-test --format "{{.Status}}"
echo "--- shallow ---"; curl -s -m 10 http://127.0.0.1:8091/healthz || true; echo
echo "--- deep ---";    curl -s -m 45 "http://127.0.0.1:8091/healthz?deep=1" || true; echo
echo "--- logs ---";    docker logs lux-telegram-test 2>&1 | tail -6