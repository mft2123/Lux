from pathlib import Path
p = Path("/tmp/lux-u11/web/src/react.css")
s = p.read_text(encoding="utf-8")

old = ".lux-select-option { display: flex; align-items: center; justify-content: space-between; gap: 10px; width: 100%; min-width: 0; min-height: 34px; padding: 0 var(--lux-control-padding-x); border: 0; border-radius: 7px; color: var(--lux-muted);"
new = ".lux-select-option { display: flex; align-items: center; justify-content: space-between; gap: 10px; width: 100%; min-width: 0; min-height: 34px; padding: 0 var(--lux-control-padding-x); border: 0; border-radius: 7px; color: var(--lux-text);"
assert s.count(old) == 1, s.count(old)
s = s.replace(old, new)

anchor = ".lux-select-option-check { flex: 0 0 auto; color: var(--lux-accent); }"
add = anchor + """

/* 原生 <select> 的弹层由系统绘制，不继承页面背景：不显式给 option 定色时，
   深色主题下会出现「白字白底 / 灰字灰底」，选项看起来是空的。
   自定义下拉（.lux-select-option）已在上方按主题取色，这里只兜底原生控件。 */
html[data-lux-theme="dark"] select option,
html[data-lux-theme="dark"] select optgroup {
  color: #f5f5f5;
  background-color: #151519;
}
html[data-lux-theme="light"] select option,
html[data-lux-theme="light"] select optgroup {
  color: #18181b;
  background-color: #ffffff;
}"""
assert s.count(anchor) == 1
s = s.replace(anchor, add)
p.write_text(s, encoding="utf-8")
print("select contrast patched")