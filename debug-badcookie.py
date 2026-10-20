import json, traceback
from p115client import P115Client

c = P115Client("UID=1; CID=2; SEID=3; KID=4")
try:
    st = c.login_status()
    print("login_status:", json.dumps(st, ensure_ascii=False)[:600])
except Exception as e:
    print("login_status raised:", type(e).__name__, str(e)[:300])

try:
    snap = c.share_snap({"share_code": "sws1yeb3wt5", "receive_code": "x3c1", "cid": 0, "limit": 100})
    print("share_snap:", json.dumps(snap, ensure_ascii=False)[:600])
except Exception as e:
    print("share_snap raised:", type(e).__name__, str(e)[:400])
    traceback.print_exc()