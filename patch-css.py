from pathlib import Path
p = Path("/tmp/lux-u11/web/src/react.css")
s = p.read_text(encoding="utf-8")
old = ".lux-button-primary { background: #f2f2f3; box-shadow: 0 8px 24px rgba(0,0,0,.25); }\n.lux-button-primary:hover { background: #fff; box-shadow: 0 12px 28px rgba(0,0,0,.35); }"
new = (
"/* 主按钮跟随强调色：切换 强调色 时全站可见；silver 是默认值，保持原来的浅色按钮。 */\n"
".lux-button-primary { color: var(--lux-accent-foreground); background: var(--lux-accent); box-shadow: 0 8px 24px rgba(0,0,0,.25); }\n"
".lux-button-primary:hover { background: color-mix(in srgb, var(--lux-accent) 88%, #fff); box-shadow: 0 12px 28px rgba(0,0,0,.35); }\n"
"html[data-lux-accent=\"silver\"] .lux-button-primary { color: #111; background: #f2f2f3; }\n"
"html[data-lux-accent=\"silver\"] .lux-button-primary:hover { background: #fff; }"
)
assert s.count(old) == 1, s.count(old)
p.write_text(s.replace(old, new), encoding="utf-8")
print("patched react.css")