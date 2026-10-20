import json
p="/vol5/1000/docker/daquanclaw/data/daquanclaw.json"
d=json.load(open(p,encoding="utf-8"))
print("top:", list(d)[:30])
for k in ("telegram","channels","tgChannels","monitor"):
    if k in d:
        v=d[k]
        print(f"{k}:", type(v).__name__, (list(v)[:15] if isinstance(v,dict) else len(v) if isinstance(v,list) else v))
        if isinstance(v,list) and v and isinstance(v[0],dict):
            print("  item keys:", sorted(v[0].keys()))
        if isinstance(v,dict):
            for kk,vv in list(v.items())[:6]:
                print("   ", kk, "=>", type(vv).__name__, (list(vv)[:12] if isinstance(vv,dict) else len(vv) if isinstance(vv,list) else ""))