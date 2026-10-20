import glob, json, os
for path in sorted(glob.glob("/config/archive-settings.json*")):
    st = os.stat(path)
    try:
        d = json.load(open(path, encoding="utf-8"))
        keys = sorted(d)
        tm = d.get("tmdb_api_key")
        cookie = (d.get("transfer115") or {}).get("cookie") or ""
        print(f"{path}  size={st.st_size} mtime={st.st_mtime:.0f}")
        print(f"   keys={keys} tmdb={'absent' if tm is None else 'len='+str(len(tm))} cookie_len={len(cookie)}")
    except Exception as e:
        print(f"{path}  unreadable: {e}")