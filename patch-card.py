from pathlib import Path
p = Path("/tmp/lux-u11/web/src/features/home/media-card-visual.tsx")
s = p.read_text(encoding="utf-8")

s = s.replace(
 'import { memo, type ReactNode } from "react";',
 'import { memo, useState, type ReactNode } from "react";'
)

old = '''  const image = imageUrl(item, landscape ? "fanart" : "poster") ?? imageUrl(item);
  const progress = playbackProgress(item);
  const isFolder = item.itemType === "FOLDER";'''
new = '''  const resolved = imageUrl(item, landscape ? "fanart" : "poster") ?? imageUrl(item);
  // 条目可能带着 imageTags 但图片接口 404（电影比剧集常见）：加载失败就退回生成封面，
  // 否则卡片上是空白，比没有封面更糟。
  const [failed, setFailed] = useState(false);
  const image = failed ? undefined : resolved;
  const progress = playbackProgress(item);
  const isFolder = item.itemType === "FOLDER";'''
assert s.count(old) == 1, s.count(old)
s = s.replace(old, new)

old_img = '<img src={image} alt="" loading="lazy" decoding="async" />'
new_img = '<img src={image} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />'
assert s.count(old_img) == 1, s.count(old_img)
s = s.replace(old_img, new_img)
p.write_text(s, encoding="utf-8")
print("card fallback patched")