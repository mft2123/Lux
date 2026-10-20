from pathlib import Path
p = Path("/tmp/lux-u11/web/src/react.css")
s = p.read_text(encoding="utf-8")
old = ".lux-horizontal-scroll-rail { position: relative; min-width: 0; }"
new = (".lux-horizontal-scroll-rail { position: relative; min-width: 0; isolation: isolate; }\n"
       "/* overflow-x:auto 会把未声明的 overflow-y 一并算成 auto，横向轨道因此成了纵向\n"
       "   可滚容器：页面纵向滚动时，相邻区块的文字会“渗”进这一行（首页媒体库那行最明显）。 */\n"
       ".lux-horizontal-scroll-viewport { overflow-y: hidden; }")
assert s.count(old) == 1
s = s.replace(old, new)
p.write_text(s, encoding="utf-8")
print("rail patched")