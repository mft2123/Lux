import json
from p115client import P115Client

cookie = json.load(open("/config/archive-settings.json", encoding="utf-8"))["transfer115"]["cookie"]
c = P115Client(cookie)
print("login:", c.login_status())

root = c.fs_files({"cid": 0, "limit": 100})
print("root type:", type(root).__name__)
text = json.dumps(root, ensure_ascii=False)
print("root keys:", list(root)[:10] if isinstance(root, dict) else text[:200])
print("root snippet:", text[:1200])

names = []
if isinstance(root, dict):
    for key in ("data", "list"):
        v = root.get(key)
        if isinstance(v, list):
            names = [i.get("n") or i.get("file_name") or i.get("name") for i in v]
        elif isinstance(v, dict) and isinstance(v.get("list"), list):
            names = [i.get("n") or i.get("file_name") or i.get("name") for i in v["list"]]
print("root names:", names)