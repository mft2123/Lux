# Lux 交接（2026-10-23 更新，给下一次会话）

> **先读这个文件，再读三份契约**：
> - `docs/archive-pipeline.md` —— 归档流水线的行为规范（代码按它执行，**改算法先改它**）
> - `docs/handoff/AUTO-LIBRARY.md` —— 自动入库契约（一个板块一个库、库内分地区）
> - `docs/handoff/CHANNEL-MONITORING.md` —— TG 频道监听 v2（**还没做**）
>
> 上一轮（2026-10-21：自动入库、一类型一库、年份→年代目录迁移）的全文在
> `docs/handoff/NEXT-SESSION-2026-10-21.md`。
>
> 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（主 checkout 在 `feat/telegram-ingest`）。
> 工作区：worktree `/tmp/lux-u11`，分支 `fix/u11-e2e`。
> 从 Windows 机器操作：`ssh -p 4888 -i ~/.ssh/id_ed25519_feiniu qiangli@192.168.8.8`；
> 长脚本走 base64 传（`runremote.ps1` 那种写法），别在 `ssh "…"` 里拼引号和 heredoc。

## 0. 2026-10-23 变更（优先读，与下文冲突时以本节为准）

代码 `9fbfa12b`，镜像 `lux:phase-a` = `d779ac5b0c231136d3864b0bd291e215079d3ecf50c64553c50d3ce1d265ab60`，
已部署到 `lux-testlib`；worktree 干净。

**板块从 8 个变 7 个：电影 / 电视剧 / 动画 / 综艺 / 纪录片 / 演唱会 / 晚会；
另外多一个 `影视 REMUX` 库（容纳所有板块的 remux 版）。**

- **取消「特摄剧」板块**（用户决定）：特摄不是内容分类，**形式决定板块**——剧集形式的特摄进
  `电视剧`，电影形式的进 `电影`。`TmdbMatch::is_tokusatsu()` 连关键词表一起删了；
  `auto_library` 的 `RETIRED_BOARDS` 让镜像里残留的 `特摄剧` 目录不再建库。
  已迁移：`假面骑士ZZZ (2025)` 从 `特摄剧/国外/2020s` 挪到 `电视剧/国外/2020s`（115 + 镜像 + 库条目），
  `特摄剧` 库已删除。现在电视剧 19 部 / 365 集。
- **三层目录可导航**（原 §6.1 已完成）：文件夹浏览的可见性判定原来只认 `VIDEO` 子项，而片名目录下是
  `MOVIE`、剧集板根下是 `SERIES`，所以 `国内/国外 → 年代 → 片名` 点不进去。现在判定同时认
  `MOVIE/SERIES/SEASON/EPISODE`；带 `parent_id` 时统一按浏览集合返回，不再被调用方的
  `MOVIE`/`MOVIE,SERIES` 扁平过滤挡住。前端 `LibraryPage` 的目录下钻对所有板块开放（合集除外）。
- **孤儿 FOLDER 自动清理**（原 §6.2 已完成）：对账每轮比对 `FOLDER.identity_key` 指向的目录是否存在，
  不存在就软删（`removed_at`）；库根 `is_available=0` 或路径没挂载时整根跳过。
- **`晚会` 分支实测通过**：合成 200KB 样本 `2026年中央广播电视总台春节联欢晚会.2026.1080p.WEB-DL`
  落在 `晚会/2026年中央广播电视总台春节联欢晚会 (2026)/`（无地区层、无年代层），
  自动建库 `晚会`（MIXED），单场识别为 MOVIE。这条样本**仍留在库里**（200KB 假文件），
  清掉的话 `晚会` 会变成空库。
- **洗版 wash 实测通过**（原 §6.4 已完成）：同名文件 1MB → 3MB，旧的进
  `洗版目录/<时间戳>-<原文件名>`，接口返回 `replaced_size` / `kept_size`。
  **踩坑**：单文件条目的判重看的是**目标文件路径（含文件名）**，不是标题目录；
  所以换个 `2160p` 名字再入库不会洗版，而是两个版本并排放进同一目录。
- **REMUX 单独成库**（用户决定，`9fbfa12b`）：remux 是同一部片的**另一个版本**，不是更好的版本，
  所以不再被洗掉，而是进 `媒体目录/影视 REMUX/<类型>/<地区>/<年代>/<片名 (年份)>/[Season NN/]`。
  - 判定只看文件名里的 `REMUX`（大小写不敏感）。**REMUX 不是分辨率**：真实库里 7546 个 REMUX
    文件中有 3406 个是 1080p。`2160p.WEB-DL` 不算 remux。
  - **整包判定**：目录包里只要有一个视频不带 `REMUX`，整包按普通版归档——否则同一季会被拆到两个库。
  - `影视 REMUX` 是一个 `MIXED` 库（电影和剧集都在里面），库内第一层仍是板块名（电影/电视剧…）。
  - 已实测：`斯巴达300勇士.2006.1080p.BluRay.REMUX` → `影视 REMUX/电影/国外/2000s/斯巴达300勇士 (2006)/`；
    `假面骑士ZZZ.S01E45.…REMUX` → `影视 REMUX/电视剧/国外/2020s/假面骑士ZZZ (2025)/Season 01/`。
    普通板块里的原片不受影响（电影仍 5 部、电视剧仍 19 部）。这两条样本也是假文件（300KB/250KB），
    留着当库样本。

剩余：TG 频道监听 v2（**用户要求放最后**）、封面/主题视觉验收（需要浏览器）、
真实库一级目录口径（`剧集`→`电视剧`、`动漫`→`动画`，等用户发话）。

## 1. 当前基线

| 项 | 值 |
| --- | --- |
| 代码 | `20fbb5e6` `feat(archive): drop the 特摄剧 board…`（前一条 `8922195a` 目录浏览 + 孤儿清理） |
| 分支 | `fix/u11-e2e`，worktree 干净 |
| 镜像 | `lux:phase-a` = `4d314149d94715761419dad02ffa3ecdbc424e1beb478bba5ee1df964b6ba688`（已部署） |
| 测试容器 | `lux-testlib`（8099→8097，admin/`Test@12345a`，healthy） |
| 115 侧车 | `lux-transfer115`（8098，healthy） |
| TG 侧车（测试） | `lux-telegram-test`（8091，24 个频道） |
| 配置 / DB | `/vol5/1000/build/archive-test-config/lux.db` |
| 115 测试库 | `/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库`（容器里 = `/testlib`） |
| 本地镜像根 | `/vol5/1000/docker/lux-library`（容器里 = `/library`） |

**重建镜像（约 4 分钟）**
```bash
cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .
```
`docker build` 成功 ≠ 改动生效：核对 `docker inspect lux-testlib --format "{{.Image}}"` 的哈希。
**重建容器**用 `/tmp/recreate3.sh`（自己从 chillposter 的 settings.json 取 TMDB key；手工重建要
`-e LUX_TMDB_API_KEY="$KEY"`，否则归档/回填全失败）。

## 2. 板块规则（唯一权威版本）

默认链：**`媒体目录/<类型>/<国内|国外>/<年代>/<片名 (年份)>/`**

| 板块 | 地区层 | 年代层 | 库 kind | 归类依据 |
| --- | --- | --- | --- | --- |
| 电影 | 有 | 有 | MOVIE | TMDB `movie` |
| 电视剧 | 有 | 有 | SERIES | TMDB `tv` |
| 动画（=动漫） | 有 | 有 | MIXED | genre `动画` |
| 综艺 | 有 | 有 | SERIES | genre `综艺`/`真人秀`/`脱口秀` |
| 纪录片 | 有 | 有 | MIXED | genre 含 `纪录` |
| **演唱会** | 有 | **无** | **MIXED** | 名称关键词/标记，不查 TMDB |
| **晚会** | **无** | **无** | **MIXED** | 名称关键词/标记，不查 TMDB |

> 2026-10-23 起**没有「特摄剧」板块**：特摄按形式归入 `电视剧` 或 `电影`（见 §0）。

三处关键实现：

- `TmdbMatch::category_label()`：`综艺|真人秀|脱口秀` → 综艺；
  genre 含 `纪录` → 纪录片；`动画` → 动画；否则 `电影`/`电视剧`（按形式，**不再看标题关键词**）。
  （TMDB 的 zh-CN genre 99 接口值是「纪录」，不是「纪录片」；也**没有**「综艺」这个 genre。）
- `archive/auto_library.rs`：`MIXED_BOARDS = ["动画", "纪录片", "演唱会", "晚会"]`（逐文件判定）；
  `RETIRED_BOARDS = ["特摄"]`（残留目录不建库）；其余按 `含"电影"→MOVIE` / `SERIES`。

`organize_special()` 负责晚会/演唱会：不查 TMDB，按名称+年份落位，镜像写 strm + 最小 NFO；
`演唱会` 按标题是否含中文判 `国内/国外`。

## 3. 本轮（10-22）做了什么

### 3.1 测试库重排：139 个片名目录去掉 `{tmdb-123}` 后缀

10-21 的回填把片名目录写成了 `片名 (年份) {tmdb-123}`，不符合命名规则。把 `媒体目录/动画` 下
139 个带后缀的目录整体挪进 `待整理/` 让流水线自己重排：

- `moved=139 failed=0 skipped=0`，清单 `/vol5/1000/build/u11-reorg-manifest.txt`
- 流水线四轮 `organized=138, failed=0, duplicates=1`，`待整理` 清空
- 唯一重复：`龙珠超 比鲁斯 (2026)` 与 `七龙珠超：比鲁斯 (2026)` 同一 tmdb id（312359）、同为 1.4G
  → `重复目录`
- 结果：`媒体目录/动画` 140 个片名目录、0 个带后缀；旧镜像目录（279 个）删到 140 个

### 3.2 修好 11 部「看不见」的剧集（+173 集）

重排后发现 11 个包只有 `tvshow.nfo`+图、没有一集 `.strm`，库里建不出条目。根因在
`parse_media_name`：为保住中文标题里的点，含 CJK 时不做 `.`→空格 切分，于是
`汤直志异.S01E01.2026.2160p…` 整串成为一个 token，季集号永远解析不出。

修法（`ef58940e`）：含 CJK 且有分隔符时先按分隔符读一遍，**只有真读到季集号才采用**，否则回落字面读法。
后续又补（`fcfe3e12`）：带发布标签（1080p/BluRay/…）且字面读法没有年份时，也用切分读法补年份
（`周华健96 香港弦全演唱会.1996.480p…` 属于这类）；`唐探.1900` 这种不带标签的标题仍保留点。

结果：+11 部剧、+173 集（汤直志异 26、我靠签到逆天改命 动态漫 48、我家隔壁盘丝洞 24、
十二层宝塔 12、宿身小人 12、万妖图录传 10、奔跑的木头 10、我叫MT：归来 10、机械飞升 10、
我的坟冢能滋养修为 6、我家小师叔不可能这么强 5）。

### 3.3 新板块跑通：纪录片 / 特摄剧

从真实库 `cp` 了 3 个文件到 `待整理`（真实库只读）：

| 样本 | 落点 |
| --- | --- |
| `Kamen Rider ZEZTZ.S01E44…mp4` | `特摄剧/国外/2020s/假面骑士ZZZ (2025)/Season 01/` |
| `And Yet The Books.S01E04…mp4` | `纪录片/国内/2010s/但是，还有书籍 (2019)/Season 01/` |
| `Kids Kingdom.2009…mkv` | `纪录片/国内/2000s/小人国 (2009)/` |

两个库都由 `archive auto library reconciliation` 自动建好并各扫一次。这轮修掉 3 个 bug：

1. `9b8d87ca` 纪录片板块不生效（TMDB 返回「纪录」，原代码精确匹配「纪录片」）。
2. `2a23d0da` 单文件包写不出 strm：`mirror_dir_for(&target)` 把文件名当目录，生成
   `…/Season 01/<集>.mp4/` 假目录 → 扫描看不到媒体 → 库空。改成按「片名目录」写（剧集落 `Season NN/`）。
3. `3a6f4636` 同样的假目录问题在「不查 TMDB」的特殊分支里又出现一次，一并修掉。

### 3.4 归档判定压测：6 条分支

每个分支只 `cp` 一个文件到 `待整理`：

| 分支 | 样本 | 结果 |
| --- | --- | --- |
| 国内电影 | 一路惊喜 (2015) | ✅ `电影/国内/2010s/一路惊喜 (2015)/`（测试库首个国内电影） |
| 多季新增 | 但是，还有书籍 S02E04 | ✅ 追加到已有剧的 `Season 02/` |
| 综艺 | 一起露营吧 S02E02 | ✅ `综艺/国内/2020s/一起露营吧 (2022)/Season 02/`，自动建库 |
| 演唱会 | 周华健96 香港弦全演唱会 | ✅ `演唱会/国内/周华健96 香港弦全演唱会 (1996)/`，库里是 MOVIE |
| 重复判定 | Kids Kingdom（与库里同尺寸） | ✅ 进 `重复目录` |
| 纪录片 genre | 一路前行（TMDB 标「纪录」） | ✅ 进 `纪录片/国内/2020s/` |

又挖出 4 个 bug：

1. `fcfe3e12` **综艺板块永远产不出来**：TMDB 没有「综艺」genre，中文综艺实际是「真人秀」/「脱口秀」。
2. `fcfe3e12` **演唱会/晚会丢年份**（见 §3.2 的第二次修复）。
3. `3a6f4636` 特殊分支单文件 sidecar 写错位置。
4. `1cf6db17` **演唱会/晚会库类型错**：按 SERIES 扫，单场演出没有 SxxExx → `UNRESOLVED` → 空库。
   改 MIXED：单场是 MOVIE、多段录制仍是剧集。

**没覆盖到的**：`晚会`（真实样本 26GB/245GB，没为压测复制；与演唱会同一分支，只差地区层）、
洗版 wash（要「同片更大版本」，测试库都是 4K remux，成本太高，仅单测覆盖）。

### 3.5 晚会/演唱会去掉年代层

用户决定（条目少）：`晚会/<片名 (年份)>/`、`演唱会/<国内|国外>/<片名 (年份)>/`，**不再有年代层**。
落点 `organize_special()`（`b3312035`），契约 `docs/archive-pipeline.md` 同步改。

### 3.6 `原因.txt` 加 BOM

失败目录每个桶的 `原因.txt` 现在是 UTF-8 + BOM（`EF BB BF`），Windows 记事本不再按 GBK 显示乱码；
正文仍是合法 UTF-8。落点 `archive.rs::write_reason_file()`（`24ee562e`）。

## 4. 当前数据状态（2026-10-22 实测）

115 `测试库/媒体目录`：

```
电影/{国内/2010s, 国外/{1980s,2000s,2010s}}
电视剧/{国内/{1990s,2010s,2020s}, 国外/2020s}
动画/{国内/{2010s,2020s}, 国外/2020s}
纪录片/国内/{2000s,2010s}
特摄剧/国外/2020s
综艺/国内/2020s
演唱会/国内                    （没有年代层）
```

本地镜像 `/vol5/1000/docker/lux-library`：结构同上（去掉 `媒体目录/` 层），每部片是
`.strm` + NFO + 海报。

| 库 | kind | 条目 |
| --- | --- | --- |
| 动画 | MIXED | SERIES 139 · SEASON 159 · EPISODE 3085 · MOVIE 1 · FOLDER 3 |
| 电影 | MOVIE | MOVIE 5 · FOLDER 9 |
| 电视剧 | SERIES | SERIES 18 · SEASON 19 · EPISODE 364 |
| 纪录片 | MIXED | MOVIE 1（小人国）· SERIES 1（但是，还有书籍）· SEASON 1 · EPISODE 1 · FOLDER 3 |
| 特摄剧 | SERIES | SERIES 1（假面骑士ZZZ）· SEASON 1 · EPISODE 1 |
| 综艺 | SERIES | SERIES 1（一起露营吧）· SEASON 1 · EPISODE 1 |
| 演唱会 | MIXED | MOVIE 1（周华健96 香港弦全演唱会）· FOLDER 2 |
| 合集 | MIXED | 空库（无根、0 条目），去留未定 |

`待整理`、`转存目录` 空；`失败目录` 7 个桶都是 10-18/10-20 的旧记录，本轮无新增失败；
`重复目录` 有 1 条新的（Kids Kingdom，同尺寸重复）。

FOLDER 条目是**目录层本身**（`国内 → 2010s → 片名 (年份)`，晚会/演唱会少一层），
页面与 Emby 接口都不把它们当媒体条目列出来。

## 5. 已定的事（别翻案）

1. **一个板块一个库**：电影/电视剧/动画/综艺/纪录片/特摄剧/演唱会/晚会，各自一个库。
2. **国内/国外、年代、片名目录是库里的目录层，不是库**；绝不按地区/产地拆库。
3. **晚会**：不分地区、无年代层；**演唱会**：分地区、无年代层；两者都不查 TMDB，按名称+年份归档，
   库类型 MIXED。
4. **动画 = 动漫**（同一板块，`动漫` 是别名，合并成一个库的第二个根）。
5. **镜像库 + `.strm` 是 Lux 原项目的做法，不改成「真实文件库」**：115 的 `媒体目录` 只放媒体；
   Lux 扫的是本地镜像（`.strm` 内容 = `LUX_ARCHIVE_CD2_MOUNT + PREFIX + 相对路径` + NFO + 海报）。
   扫描快、云端库不被 sidecar 污染、媒体只有一份。`media_url()` 只在路径落在 CD2 挂载下时才写 strm。
6. **真实库 `影视库/媒体目录` 全程只读**：不扫、不移动、不改名、不删除，除非用户单独发话。
   截至本文件，两轮验证只从真实库 `cp` 过 6 个文件到 `待整理`（特摄 1、纪录片 2、国内电影 1、综艺 1、
   演唱会 1），没有修改原文件。
7. **以后新内容一律只进 `待整理`（或 `转存目录`）**，不手工往 `媒体目录` 塞；要重排也是挪进 `待整理`。

## 6. 未完成（按优先级）

1. ~~三层目录在界面上不可导航~~ —— **2026-10-23 完成**（`8922195a`，见 §0）。
2. ~~孤儿 FOLDER 不自动清理~~ —— **2026-10-23 完成**（`8922195a`，见 §0）。
3. ~~`晚会` 没跑过真实样本~~ —— **2026-10-23 用合成样本实测通过**（见 §0）。
4. ~~洗版 wash 只有单测~~ —— **2026-10-23 实测通过**（见 §0）。
5. ~~特摄剧判定靠标题清单~~ —— **作废**：特摄剧板块已取消，按形式归类（`20fbb5e6`）。
6. **真实库一级目录与 Lux 口径不同**：真实库是 `剧集/动漫/电影/综艺/纪录片/晚会/演唱会/特摄剧/…`，
   Lux 写 `电视剧/动画`；`动漫` 已有别名，`剧集` 还没做。**真实库仍不许动**，方案等用户发话。
   注意真实库里那个 `特摄剧` 板（3122 个 strm）是 chill 侧的历史归档，Lux 不再产出该板块，
   但也不会去动它。
7. **TG 频道监听 v2 的 Lux 侧**（契约 `CHANNEL-MONITORING.md`）：代取频道 → 设置 schema → 每频道游标
   + 判定矩阵；`manual` 是派生值，不是独立开关。工作量最大，**用户要求放最后**。
8. **封面/主题视觉验收**：本机无浏览器，10-20 那批 UI 改动只验到编译+接口（10-23 的目录下钻同样只验到接口）。
9. 电影库只有 5 部（全在国外），样本量小；`合集` 空库去留未定（它是内置的全局合集库，不是自动入库建的）。
10. ~~多版本共存 / REMUX 分库~~ —— **2026-10-23 完成**（`9fbfa12b`，见 §0）。
    后续可考虑：remux 库要不要再按分辨率分层、`影视 REMUX` 是否要显示别名（现在库里就叫「影视 REMUX」）。

## 7. 踩坑（必看）

1. **TMDB 的 zh-CN 类型名要按接口值匹配**：genre 99 是「纪录」不是「纪录片」；没有「综艺」，
   中文综艺是「真人秀」/「脱口秀」。加板块前先
   `curl "https://api.themoviedb.org/3/tv/<id>?api_key=$KEY&language=zh-CN"` 看真实 `genres`。
2. **中文标题 + 点号**：`parse_media_name` 对 CJK 默认不切点，`中文名.S01E01…` 会整串粘住 → 季集号
   丢失 → 没 `.strm` → 整部消失。现在「读到季集号」或「带发布标签能补年份」才用切分读法，别再改回去。
3. **单文件条目不能当目录包写镜像**：`mirror_dir_for(&target)` 对文件会把文件名带进路径，生成假目录。
   单文件一律按「片名目录」传给 `MetadataWriter`（剧集是 `Season NN` 的父目录）。
4. **挪走片名目录后必须删旧镜像目录**：不删就新旧并存，旧的 strm 指向不存在的 115 路径。
5. **删镜像目录要精确匹配 `*{tmdb-*`**，别用宽泛通配（标题本身带括号和中文）。
6. **115 上 `mv` 是逐条 API 调用**：139 个目录好几分钟；用清单文件逐条记录，别 `mv *`。
7. **重排前先查重名**：同一 `片名 (年份)` 可能同时在国内和国外，直接 mv 会撞进 `待整理`。
8. **回填会跳过已有 `tvshow.nfo` 的目录**：要重写 sidecar，先删它的镜像目录。
9. **改库 kind 要删库重建**：`SERIES` → `MIXED` 没有接口改；DELETE 库后重启容器触发对账重建
   （归档 run 只有在 `organized` 非空时才触发对账）。
10. **`docker build` 成功 ≠ 改动生效**：核对镜像哈希 + `docker inspect`。
11. **别用 `git branch -f … HEAD`**（主 checkout 的 HEAD 会把主干指回旧提交），给显式 SHA。
12. `tests/admin_health.rs` 在干净 HEAD 也失败（`maxConnections` 期望 8、实际 24），环境相关，别当回归。
13. **单文件条目的洗版看的是「目标文件路径」，不是标题目录**：`target = title_dir/<原文件名>`，
    所以只有**同名**且更大才会洗版；换个 `2160p` 名字再入库是"两个版本并排共存"，不会洗。
    想验证 wash 必须同名改大小（1MB→3MB 即可，不必 4K remux）。
14. **取消板块不只是改分类**：`auto_library` 按镜像一级目录建库，目录还在就会复活板块；
    删板块要同时加 `RETIRED_BOARDS` + 清镜像目录 + 删库（`DELETE /api/v1/admin/libraries/{id}`）。
15. **`合集` 是内置的全局合集库**（`GLOBAL_COLLECTION_LIBRARY_ID`），不是自动入库建的；
    删了会在下次启动时回来，别当空库清理。
16. **TMDB 偶发 `error sending request`（不是 429）**：同一批里 TV 查询成功、Movie 查询失败，
    但同一个 URL 用 curl（host 和容器内）都是 200。先别怀疑代码——把条目从 `失败目录`
    捞回 `待整理` 重跑通常就好了。造测试样本时优先挑库里已有、且 TMDB 稳定的片名。

## 8. 常用验收命令

```bash
B=http://127.0.0.1:8099
curl -s -c /tmp/c.jar -X POST $B/api/v1/auth/login -H 'content-type: application/json' \
  -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)

# 库 / 库条目
curl -s -b /tmp/c.jar $B/api/v1/libraries
curl -s -b /tmp/c.jar "$B/api/v1/libraries/<lib_id>/items?limit=20"

# 回填镜像（缺 sidecar 时）：先删对应镜像目录，再跑
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" \
  "$B/api/v1/admin/archive/metadata?dry_run=false" \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);print({k:len(d.get(k) or []) for k in ("created","skipped","failed")})'

# 重扫某库 / 手动跑归档（dry_run 默认 true）
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" "$B/api/v1/admin/libraries/<lib_id>/scan"
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" "$B/api/v1/admin/archive/run?dry_run=false"

# 流水线 / 自动入库日志
docker logs lux-testlib 2>&1 | grep -E "archive pipeline run completed|archive auto library"

# 库 / 根 / 扫描 / 条目
python3 - <<'PY'
import sqlite3
c = sqlite3.connect('/vol5/1000/build/archive-test-config/lux.db')
print(list(c.execute('select name,kind from libraries order by name')))
print([r[0] for r in c.execute('select display_path from library_roots order by display_path')])
print(list(c.execute('select status,count(*) from scan_jobs group by status')))
print(list(c.execute("select l.name,m.item_type,count(*) from media_items m join libraries l on l.id=m.library_id where m.removed_at is null group by 1,2 order by 1,2")))
PY

# 单测（归档相关，约 2 分钟）
docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home -v /tmp/lux-u11:/src -v /vol5/1000/build/target:/src/target -w /src \
  rust:1.94-test bash -c "cargo test --locked --lib archive"
```

补 sidecar 的完整动作（新目录/改过层级时）：

```bash
# 1) 删旧镜像目录（只删要重建的那几条，别通配整个板块）
rm -rf /vol5/1000/docker/lux-library/<板块>/<地区>/<年代>/<片名 (年份)>
# 2) 回填
# 3) 重扫该库
```

## 9. 建议的下一步顺序（2026-10-23 重排）

1. 真实库方案（一级目录改名映射 `剧集`→`电视剧`、`动漫`→`动画`）——**等用户发话再动**。
2. 封面/主题视觉验收（需要一台有浏览器的机器）。
3. **TG 频道监听 v2**（契约已成，工作量最大）——**用户要求放最后**。
