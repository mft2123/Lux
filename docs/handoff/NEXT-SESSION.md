# Lux 交接（2026-10-22 收尾，给下一次会话）

> **先读这个文件，再读三份契约**：
> - `docs/archive-pipeline.md` —— 归档流水线的行为规范（代码按它执行，改算法先改它）
> - `docs/handoff/AUTO-LIBRARY.md` —— 自动入库契约（一个板块一个库、库内分地区）
> - `docs/handoff/CHANNEL-MONITORING.md` —— TG 频道监听 v2（还没做）
>
> 上一轮（2026-10-21：自动入库、一类型一库、年份→年代目录迁移）的全文在
> `docs/handoff/NEXT-SESSION-2026-10-21.md`，需要背景时再翻。
>
> 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（主 checkout 在 `feat/telegram-ingest`）。
> 工作区：worktree `/tmp/lux-u11`，分支 `fix/u11-e2e`（从这台 Windows 机器用
> `ssh -p 4888 -i ~/.ssh/id_ed25519_feiniu qiangli@192.168.8.8` 操作；脚本用
> `runremote.ps1` 那种 base64 方式传，别直接拼引号）。

## 1. 当前基线

| 项 | 值 |
| --- | --- |
| 代码提交 | `2a23d0da` `fix(archive): write sidecars for a single moved file into its title dir` |
| 本轮其它提交 | `9b8d87ca` 纪录片「纪录」genre · `ef58940e` 中文标题点号解析 · `cebc0d8b` 本文件 · `1ece2a0b` 纪录片/特摄剧板块 · `24ee562e` `原因.txt` 加 BOM |
| 分支状态 | `fix/u11-e2e`，worktree 干净 |
| 镜像 | `lux:phase-a` = `d7fa41f555d5…`（已部署，容器在跑） |
| 测试容器 | `lux-testlib`（8099→8097，admin/`Test@12345a`，healthy） |
| 115 侧车 | `lux-transfer115`（8098，healthy） |
| TG 侧车（测试） | `lux-telegram-test`（8091，24 个频道） |
| 测试配置 | `/vol5/1000/build/archive-test-config` |
| 115 测试库挂载 | `/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库`（= 容器里的 `/testlib`） |
| 本地镜像根 | `/vol5/1000/docker/lux-library`（= 容器里的 `/library`） |

**容器需要 TMDB key**（否则归档/回填全失败）。重建容器用 `/tmp/recreate3.sh`，它自己从
chillposter 的 settings.json 取 key；手工重建时要加 `-e LUX_TMDB_API_KEY="$KEY"`。

**重建镜像**：`cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .`
（约 4 分钟）。`docker build` 成功不代表改动进包：看**镜像哈希是否变化**。

## 2. 本轮（10-22）做了什么

### 2.1 板块规则拍死（用户确认）

**所有板块统一三层**：`<类型>/<国内|国外>/<年代>/<片名 (年份)>/`。

| 板块 | 是否分国内/国外 | 库 kind |
| --- | --- | --- |
| 电影 | 分 | MOVIE |
| 电视剧 / 综艺 / 特摄剧 / 演唱会 | 分 | SERIES |
| 动画 / 纪录片 | 分 | MIXED（板块里既有电影也有剧集，扫描按文件逐个判定） |
| **晚会** | **不分**（`晚会/<年代>/<片名 (年份)>/`） | SERIES |

代码落点：`TmdbMatch::category_label()`（`综艺`/`纪录片`/`特摄剧`/`动画` → 否则 电影/电视剧）、
`TmdbMatch::is_tokusatsu()`（TMDB 没有「特摄」类型，按标题里的系列名清单判：奥特曼/假面骑士/
超级战队/牙狼/铠甲勇士/特摄/ultraman/kamen rider/super sentai/garo）、
`archive/auto_library.rs` 的 `MIXED_BOARDS = ["动画", "纪录片"]`、
`backfill_metadata` 的 `series_category` / `mixed_category`。

### 2.2 测试库重排（139 个片名目录）

背景：10-21 的回填把片名目录写成了 `片名 (年份) {tmdb-123}`，不符合 `<片名 (年份)>` 的命名。
本轮把 `媒体目录/动画` 下 139 个带后缀的片名目录整体挪进 `待整理/`，让流水线自己重排：

- `moved=139 failed=0 skipped=0`，清单在 `/vol5/1000/build/u11-reorg-manifest.txt`
- 流水线四轮：`organized=138, failed=0, duplicates=1`，`待整理` 清空
- 唯一重复：`龙珠超 比鲁斯 (2026)` 与 `七龙珠超：比鲁斯 (2026)` 同一 tmdb id（312359）、同为 1.4G → `重复目录`
- 结果：`媒体目录/动画` **140 个片名目录、0 个带 `{tmdb-}`**

### 2.3 顺手修掉的真 bug：11 部片子看不见

重排后发现 11 个包只有 `tvshow.nfo`+图，没有一集 `.strm`，扫描建不出条目。根因在
`parse_media_name`：为了保住中文标题里的点，含 CJK 时**不**做 `.`→空格 切分，于是
`汤直志异.S01E01.2026.2160p…` 整串粘成一个 token，季集号永远解析不出来。

修法（`ef58940e`）：含 CJK 且有分隔符时先按分隔符读一遍，**只有真读到季集号才采用**，
否则回落到字面读法（`唐探.1900` 这种标题里的点不会被吃掉）。

修完删掉这 11 个空镜像目录 → 回填 → 重扫：**+11 部剧、+173 集**
（汤直志异 26、我靠签到逆天改命 动态漫 48、我家隔壁盘丝洞 24、十二层宝塔 12、宿身小人 12、
万妖图录传 10、奔跑的木头 10、我叫MT：归来 10、机械飞升 10、我的坟冢能滋养修为 6、
我家小师叔不可能这么强 5）。

### 2.4 `原因.txt` 加 BOM

失败目录里每个桶的 `原因.txt` 现在是 UTF-8 + BOM（`EF BB BF`），Windows 记事本不再按 GBK
显示成乱码；正文仍是合法 UTF-8。落点 `archive.rs::write_reason_file()`。

### 2.5 新板块端到端验证（纪录片 / 特摄剧）

从真实库 `cp` 了 3 个文件到 `待整理`（只读真实库），跑通「归档 → 镜像 → 自动建库 → 扫描」：

| 样本 | 落点 |
| --- | --- |
| `Kamen Rider ZEZTZ.S01E44…mp4` | `特摄剧/国外/2020s/假面骑士ZZZ (2025)/Season 01/` |
| `And Yet The Books.S01E04…mp4` | `纪录片/国内/2010s/但是，还有书籍 (2019)/Season 01/` |
| `Kids Kingdom.2009…mkv` | `纪录片/国内/2000s/小人国 (2009)/` |

日志里 `archive auto library reconciliation` 依次 `created:["特摄剧"]`、`created:["纪录片"]`，
两个库自动建好并各扫一次，条目都进库了。

这轮验证挖出并修掉三个 bug（都不只影响这两个板块）：

1. **`ef58940e` 中文标题粘点号** → 见 §2.3。
2. **`9b8d87ca` 纪录片板块不生效**：TMDB 的 zh-CN genre 99 在接口上返回「纪录」，站点上写「纪录片」；
   原来按「纪录片」精确匹配，样本落到 `电影` / `电视剧`。改成按「纪录」匹配，两种写法都覆盖。
3. **`2a23d0da` 单文件包写不出 strm**：一集单文件没有自己的目录，`mirror_dir_for(&target)` 会把
   文件名带进镜像路径，生成 `…/Season 01/<集>.mp4/` 这种假目录（里面是海报+NFO），扫描器看不到
   媒体 → 特摄剧库扫出 0 条。改成跟回填一致：单文件按「片名目录」写，剧集落进 `Season NN/`。

## 3. 当前数据状态（2026-10-22 实测）

115 `测试库/媒体目录`：

```
电影/国外/{1980s,2000s,2010s}                     （5 部，测试库没有国内电影）
电视剧/{国内/{1990s,2010s,2020s}, 国外/2020s}      （17 部）
动画/{国内/{2010s,2020s}, 国外/2020s}              （140 个片名目录）
```

本地镜像 `/vol5/1000/docker/lux-library`：结构同上（去掉 `媒体目录/` 层），每部片是
`.strm` + NFO + 海报；动画 140 个目录全部有 `tvshow.nfo`，没有残留的后缀目录。

| 库 | kind | 条目 |
| --- | --- | --- |
| 动画 | MIXED | SERIES 139 · SEASON 159 · EPISODE 3085 · MOVIE 1 · FOLDER 3 |
| 电影 | MOVIE | MOVIE 5 · FOLDER 9 |
| 电视剧 | SERIES | SERIES 18 · SEASON 19 · EPISODE 364 |
| 纪录片 | MIXED | MOVIE 1（小人国）· SERIES 1（但是，还有书籍）· SEASON 1 · EPISODE 1 · FOLDER 3 |
| 特摄剧 | SERIES | SERIES 1（假面骑士ZZZ）· SEASON 1 · EPISODE 1 |

`待整理`、`转存目录` 都是空；`失败目录` 7 个桶全是 10-18/10-20 的旧记录，本轮没有新增失败。

那 3 条（动画）和 9 条（电影）FOLDER 是**目录层本身**（`国内 → 2010s → 片名 (年份)`），
按三层规则它们该存在，不用删；页面和 Emby 接口都不会把它们当媒体条目列出来。

## 4. 已定的事（别翻案）

1. **一个板块一个库**，`电影`/`电视剧`/`动画`/`综艺`/`纪录片`/`特摄剧`/`演唱会`/`晚会` 各自一个库。
2. **国内/国外、年代、片名目录是库里的目录层，不是库**；绝不按地区/产地拆库。
3. **晚会不分地区**，其余板块都分；晚会/演唱会不查 TMDB，按名称+年份归档。
4. **动画 = 动漫**（同一板块，`动漫` 是别名，合并成一个库的第二个根）。
5. **真实库 `影视库/媒体目录` 全程只读**：不扫、不复制、不改名、不删除，除非用户单独发话。
   本轮验新板块时只从真实库 `cp` 了 3 个文件到 `待整理`（一集特摄、一集纪录片、一部纪录电影），
   没有移动或修改。
6. **以后新内容一律只进 `待整理`（或 `转存目录`）**，不再手工往 `媒体目录` 里塞东西；要重排也是
   挪进 `待整理` 让它自己跑。
7. **镜像库 + `.strm` 是 Lux 原项目的做法，不改成"真实文件库"**：115 的 `媒体目录` 只放媒体，
   Lux 扫描的是本地镜像 `/vol5/1000/docker/lux-library`，里面是 `.strm`（内容 =
   `LUX_ARCHIVE_CD2_MOUNT + PREFIX + 相对路径`）+ NFO + 海报。扫描快、云端库不被 sidecar 污染、
   媒体只有一份。`media_url()` 只在路径落在 CD2 挂载下时才写 strm，普通本地文件不会生成。

## 5. 未完成（按优先级）

1. **三层目录在界面上不可导航**：`CATALOG_FOLDER_BROWSE_VISIBLE_PREDICATE` 只认 `VIDEO` 子项，
   电影是 `MOVIE`、剧集是 `SERIES`，所以点不进 `国内/国外 → 年代 → 片名` 这三层（数据在、
   进度不受影响）。要做的话是改判定 + 让 `parent_scope` 放行 `MOVIE/SERIES/SEASON/EPISODE`。
2. **纪录片 / 特摄剧只验了 1-2 个样本**：`docs/archive-pipeline.md` 与
   `docs/handoff/AUTO-LIBRARY.md` 都写好了，端到端（归档 → 镜像 → 自动建库 → 扫描）跑通，
   但样本量小，更多片名/多季/多地区的组合还要再压一压。
3. **特摄剧判定靠标题清单**，真实片库接进来前要按实际名称补 `is_tokusatsu()` 的名单。
   实测已覆盖：`假面骑士ZZZ`；待补：奥特曼/超级战队/牙狼/铠甲勇士的更多命名变体。
4. **真实库一级目录与 Lux 口径不同**：真实库是 `剧集/动漫/电影/综艺/纪录片/晚会/演唱会/特摄剧/…`，
   Lux 写 `电视剧/动画`；`动漫` 已支持别名，`剧集` 还没做。**真实库仍然不许动**，方案等用户发话。
5. **TG 频道监听 v2 的 Lux 侧**（契约 `CHANNEL-MONITORING.md`）：代取频道 → 设置 schema →
   每频道游标 + 判定矩阵。`manual` 是派生值，不是独立开关。工作量最大。
6. **封面/主题视觉验收**：本机无浏览器，10-20 那批 UI 改动只验到编译+接口。
7. 电影库只有 5 部（全是国外），没有国内样本；`合集` 空库（MIXED、无根、0 条目）还没定去留。

## 6. 踩坑（必看）

1. **中文标题 + 点号切分**：`parse_media_name` 对 CJK 默认不切点，`中文名.S01E01.…` 会整串粘住
   → 季集号丢失 → 没 `.strm` → 库里整部消失。已按「读到季集号才用切分读法」修好，但同样的
   思路改解析时别再退回去。
2. **挪走片名目录后必须删旧镜像目录**：镜像按旧的 115 路径存在，不删就会新旧两份并存，
   旧的那份 `strm` 指向已经不存在的 115 路径。
3. **删镜像目录要精确匹配 `*{tmdb-*`**，别用宽泛通配，`动画` 下很多标题本身带括号和中文。
4. **115 上 `mv` 是逐条 API 调用**，139 个目录要跑好几分钟；用清单文件 + 逐条记录，
   别一条命令 `mv *`。
5. **重排前先查重名**：同一 `片名 (年份)` 可能同时出现在国内和国外，直接 mv 会撞进 `待整理`。
6. **回填会跳过已有 `tvshow.nfo` 的目录**：要重写某部剧的 sidecar，先删它的镜像目录。
7. **TMDB 的 zh-CN 类型名要按接口值匹配**：genre 99 是「纪录」不是「纪录片」；别按网站文案写死。
   加新板块前先 `curl https://api.themoviedb.org/3/tv/<id>?language=zh-CN` 看实际 `genres`。
8. **单文件条目不能当目录包写镜像**：`mirror_dir_for(&target)` 对文件会把文件名带进路径；
   单文件一律按「片名目录」（剧集是 `Season NN` 的父目录）传给 `MetadataWriter`。
9. **`docker build` 成功 ≠ 改动生效**：看镜像哈希 + `docker inspect lux-testlib --format "{{.Image}}"`。
10. **别用 `git branch -f … HEAD`**（主 checkout 的 HEAD 会把主干指回旧提交），要给显式 SHA。
11. `tests/admin_health.rs` 在干净 HEAD 也失败（`maxConnections` 期望 8、实际 24），环境相关，别当回归。

## 7. 常用验收命令

```bash
B=http://127.0.0.1:8099
curl -s -c /tmp/c.jar -X POST $B/api/v1/auth/login -H 'content-type: application/json' \
  -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)

# 库列表 / 库条目
curl -s -b /tmp/c.jar $B/api/v1/libraries
curl -s -b /tmp/c.jar "$B/api/v1/libraries/<lib_id>/items?limit=20"

# 回填镜像（缺 sidecar 时）
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" \
  "$B/api/v1/admin/archive/metadata?dry_run=false" | python3 -c 'import json,sys;d=json.load(sys.stdin);print({k:len(d.get(k) or []) for k in ("created","skipped","failed")})'

# 重扫某个库
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" "$B/api/v1/admin/libraries/<lib_id>/scan"

# 归档流水线 / 自动入库
docker logs lux-testlib 2>&1 | grep -E "archive pipeline run completed|archive auto library"

# 库 / 根 / 扫描 / 条目
python3 - <<'PY'
import sqlite3
c = sqlite3.connect('/vol5/1000/build/archive-test-config/lux.db')
print(list(c.execute('select name,kind from libraries order by name')))
print([r[0] for r in c.execute('select display_path from library_roots order by display_path')])
print(list(c.execute('select status,count(*) from scan_jobs group by status')))
print(list(c.execute("select l.name,m.item_type,count(*) from media_items m join libraries l on l.id=m.library_id where m.removed_at is null group by 1,2")))
PY

# 单测（只跑归档相关，约 1-2 分钟）
docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home -v /tmp/lux-u11:/src -v /vol5/1000/build/target:/src/target -w /src \
  rust:1.94-test bash -c "cargo test --locked --lib archive"
```

## 8. 建议的下一步顺序

1. 补纪录片/特摄剧的测试样本，把新板块跑通一遍（归档 → 镜像 → 自动建库 → 扫描）。
2. 定「三层目录要不要在界面上可点进去」，要做就改文件夹可见性判定那一段。
3. TG 频道监听 v2（契约已写好，工作量最大）。
4. 真实库的方案（一级目录改名映射：`剧集`→`电视剧`、`动漫`→`动画` 别名）——**等用户发话再动**。
5. 找一台有浏览器的机器做封面/主题的视觉验收。
