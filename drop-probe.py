import json, os
p = "/config/archive-settings.json"
d = json.load(open(p, encoding="utf-8"))
d.pop("tmdb_api_key", None)
tmp = p + ".tmp"
with open(tmp, "w", encoding="utf-8") as fh:
    json.dump(d, fh, ensure_ascii=False, indent=2)
os.chmod(tmp, 0o600)
os.replace(tmp, p)
print("keys now:", sorted(json.load(open(p, encoding="utf-8"))))