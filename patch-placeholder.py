from pathlib import Path
p = Path("/tmp/lux-u11/web/src/features/home/media-display.tsx")
s = p.read_text(encoding="utf-8")

old = '''export function MediaPlaceholder({ item }: { item: MediaItem }) {
  const pending = item.localMetadataPending === true;
  return (
    <div className="lux-media-placeholder">
      {item.itemType === "FOLDER" ? <Folder size={30} aria-hidden="true" /> : pending ? (
        <span className="lux-media-placeholder-status" role="status" aria-label="正在读取本地海报和元数据">
          <LoaderCircle className="lux-media-placeholder-spinner" size={24} aria-hidden="true" />
        </span>
      ) : null}
      <span>{mediaTitle(item)}</span>
    </div>
  );
}'''
new = '''function generatedCoverKind(item: MediaItem): GeneratedCoverKind {
  // 动画暂时并入剧集：等有了 TMDB genre 16 的数据再细分。
  return item.itemType === "MOVIE" ? "movie" : "series";
}

export function MediaPlaceholder({ item }: { item: MediaItem }) {
  const pending = item.localMetadataPending === true;
  if (item.itemType === "FOLDER") {
    return (
      <div className="lux-media-placeholder">
        <Folder size={30} aria-hidden="true" />
        <span>{mediaTitle(item)}</span>
      </div>
    );
  }
  // 没有海报时画一张：比灰底占位更容易辨认，也不依赖网络。
  return (
    <div className="lux-media-placeholder is-generated">
      <GeneratedCover
        title={mediaTitle(item)}
        year={item.productionYear}
        kind={generatedCoverKind(item)}
      />
      {pending ? (
        <span className="lux-media-placeholder-status" role="status" aria-label="正在读取本地海报和元数据">
          <LoaderCircle className="lux-media-placeholder-spinner" size={24} aria-hidden="true" />
        </span>
      ) : null}
    </div>
  );
}'''
assert s.count(old) == 1, s.count(old)
s = s.replace(old, new)
s = 'import { GeneratedCover, type GeneratedCoverKind } from "../../components/GeneratedCover";\n' + s
p.write_text(s, encoding="utf-8")

css = Path("/tmp/lux-u11/web/src/components/generated-cover.css")
c = css.read_text(encoding="utf-8")
c += """
/* 落在原有占位容器里时撑满整块，并把加载指示叠在上面 */
.lux-media-placeholder.is-generated { position: relative; display: block; padding: 0; background: transparent; }
.lux-media-placeholder.is-generated > .lux-generated-cover { position: absolute; inset: 0; }
.lux-media-placeholder.is-generated .lux-media-placeholder-status { position: absolute; inset: 0; display: grid; place-items: center; }
"""
css.write_text(c, encoding="utf-8")
print("wired placeholder")