import json
from p115client import P115Client

cookie = json.load(open("/config/archive-settings.json", encoding="utf-8"))["transfer115"]["cookie"]
c = P115Client(cookie)

def items(payload):
    data = payload.get("data") if isinstance(payload, dict) else None
    if isinstance(data, dict):
        data = data.get("list")
    return data if isinstance(data, list) else []

root = items(c.fs_files({"cid": 0, "limit": 100}))
testlib = next(i for i in root if i.get("n") == "测试库")
subs = items(c.fs_files({"cid": testlib["cid"], "limit": 100}))
target = next(i for i in subs if i.get("n") == "转存目录")
print("转存目录 cid:", target["cid"])
for item in items(c.fs_files({"cid": target["cid"], "limit": 100})):
    print("  -", item.get("n"), "dir" if not item.get("fid") else "file", "size", item.get("s"))