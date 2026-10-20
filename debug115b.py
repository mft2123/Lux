import json
from p115client import P115Client

cookie = json.load(open("/config/archive-settings.json", encoding="utf-8"))["transfer115"]["cookie"]
c = P115Client(cookie)

def items(payload):
    data = payload.get("data") if isinstance(payload, dict) else None
    return data if isinstance(data, list) else []

root = items(c.fs_files({"cid": 0, "limit": 100}))
testlib = next(i for i in root if i.get("n") == "测试库")
print("测试库 keys:", sorted(testlib.keys()))
print("测试库 cid/fid:", testlib.get("cid"), testlib.get("fid"), "m:", testlib.get("m"))

children = items(c.fs_files({"cid": testlib["cid"], "limit": 50}))
print("children:", [(i.get("n"), "fid" in i, i.get("cid"), i.get("m")) for i in children])

# find a folder that holds files, to compare a file item
for folder in root:
    if not folder.get("cid") or folder.get("n") in ("云下载", "最近接收"):
        continue
    subs = items(c.fs_files({"cid": folder["cid"], "limit": 20}))
    files = [i for i in subs if i.get("fid")]
    if files:
        f = files[0]
        print("FILE sample from", folder.get("n"), "keys:", sorted(f.keys()))
        print("  fid:", f.get("fid"), "cid:", f.get("cid"), "n:", f.get("n"), "s:", f.get("s"), "m:", f.get("m"))
        break