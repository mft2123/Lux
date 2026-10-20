import json
from p115client import P115Client

real = json.load(open("/config/archive-settings.json", encoding="utf-8"))["transfer115"]["cookie"]
good = P115Client(real)
st = good.login_status()
print("GOOD login_status type:", type(st).__name__, "value:", json.dumps(st, ensure_ascii=False)[:400] if not isinstance(st, bool) else st)

bad = P115Client("UID=1; CID=2; SEID=3; KID=4")
print("BAD login_status:", bad.login_status())
try:
    r = bad.share_receive({"share_code": "sws1yeb3wt5", "receive_code": "x3c1", "file_id": "1", "cid": "0", "is_check": 1})
    print("BAD share_receive returned:", json.dumps(r, ensure_ascii=False)[:300])
except Exception as e:
    print("BAD share_receive raised:", type(e).__name__, "|", str(e)[:300])