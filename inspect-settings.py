import json
p = "/config/archive-settings.json"
d = json.load(open(p, encoding="utf-8"))
print("top-level keys:", sorted(d))
t = d.get("transfer115", {})
print("transfer115 keys:", {k: (len(v) if isinstance(v, str) else v) for k, v in t.items()})
tm = d.get("tmdb_api_key")
print("tmdb_api_key:", "absent" if tm is None else f"len={len(tm)}" if isinstance(tm, str) else type(tm).__name__)
tg = d.get("telegram", {})
print("telegram keys:", sorted(tg) if isinstance(tg, dict) else type(tg).__name__)
print("telegram.api_key len:", len(tg.get("api_key") or "") if isinstance(tg, dict) else "-")