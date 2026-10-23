---
artifact_contract: "ce-handoff/v1"
created_at: "2026-10-23T14:10:00Z"
title: "Lux 归档流水线：目录浏览、孤儿清理、特摄取消、REMUX 与国产短剧分库"
summary: "2026-10-23 一轮：修好三层目录下钻与孤儿 FOLDER 清理，取消特摄剧板块改按形式归类，新增 影视原盘（REMUX）与 国产短剧 两个专用库；剩视觉验收与 TG 监听"
keywords: ["lux", "archive", "catalog", "remux", "国产短剧", "board", "nas", "emby", "docker"]
cwd: "C:\\Users\\liqiang\\AppData\\Roaming\\dsh-desktop\\harness"
resume_focus: "浏览器里过一遍 VISUAL-CHECK-2026-10-23.md 的十条；之后按用户要求把 TG 频道监听 v2 放最后"
repository: "lux"
branch: "fix/u11-e2e"
head: "1272d5f5"
worktree_path: "/tmp/lux-u11"
---

# Lux 交接（2026-10-23）

> 下一会话先读这份，再按需翻两份契约：
> - `docs/archive-pipeline.md` —— 归档行为规范（**改算法先改它**）
> - `docs/handoff/AUTO-LIBRARY.md` —— 自动入库契约（一板块一库、库内分目录）
>
> 历史：`docs/handoff/NEXT-SESSION-2026-10-21.md`（更早一轮）。
> 视觉验收单：`docs/handoff/VISUAL-CHECK-2026-10-23.md`（**还没执行**）。

## 1. 怎么连、怎么跑

- 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（主 checkout 在 `feat/telegram-ingest`）
- 工作区：worktree `/tmp/lux-u11`，分支 `fix/u11-e2e`，本轮结束时 worktree 干净
- 从 Windows：`ssh -p 4888 -i ~/.ssh/id_ed25519_feiniu qiangli@192.168.8.8`
- 长脚本别在 `ssh "…"` 里拼引号/heredoc：本地拼好脚本 → base64 → `echo <b64> | base64 -d | bash`
- 测试容器：`lux-testlib`（宿主 8099 → 容器 8097，`admin` / `Test@12345a`）
- 重建镜像：`cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .`（约 4 分钟）
- 重建容器：`/tmp/recreate3.sh`（自己从 chillposter 的 settings.json 取 TMDB key；手工重建必须带 `-e LUX_TMDB_API_KEY="$KEY"`）
- **`docker build` 成功 ≠ 生效**：核对 `docker inspect lux-testlib --format "{{.Image}}"`

本轮部署的镜像：`lux:phase-a` = `2507d17f188da1b7bd8f6dde0303873b93afe5e40556a736d096d745b9f69c41`。
本轮功能性提交：`8922195a`（目录浏览 + 孤儿清理）、`20fbb5e6`（取消特摄剧）、
`9fbfa12b` + `36293df6`（REMUX 库与命名）、`d1a8beec` + `cbde11c0`（国产短剧）。

## 2. 当前板块（权威）

默认链：`媒体目录/<类型>/<国内|国外>/<年代>/<片名 (年份)>/`

| 板块 | 地区层 | 年代层 | kind | 判定 |
| --- | --- | --- | --- | --- |
| 电影 | 有 | 有 | MOVIE | TMDB movie |
| 电视剧 | 有 | 有 | SERIES | TMDB tv |
| 动画（=动漫） | 有 | 有 | MIXED | genre `动画` |
| 综艺 | 有 | 有 | SERIES | genre 综艺/真人秀/脱口秀 |
| 纪录片 | 有 | 有 | MIXED | genre 含 `纪录` |
| 演唱会 | 有 | 无 | MIXED | 名称关键词，不查 TMDB |
| 晚会 | 无 | 无 | MIXED | 名称关键词，不查 TMDB |
| **REMUX** | 有 | 有 | MIXED | 文件名含 `REMUX`；库显示名 **影视原盘** |
| **国产短剧** | 无 | 有 | MIXED | 放进 `待整理/国产短剧/` 即归此板，不查 TMDB |

**特摄剧板块已取消**（2026-10-23 用户决定）：特摄不是内容分类，形式决定板块——剧集形式的进
`电视剧`、电影形式的进 `电影`。`is_tokusatsu()` 已删；`auto_library` 的 `RETIRED_BOARDS`
让残留的 `特摄剧` 目录不再建库。

## 3. 这一轮改了什么（都是用户决定，不是我的推断）

1. **三层目录可下钻**（`8922195a`）：`CATALOG_FOLDER_BROWSE_VISIBLE_PREDICATE` 原来只认 `VIDEO`
   子项，而片名目录下是 `MOVIE`、剧集板根下是 `SERIES`，所以 `国内/国外 → 年代 → 片名` 点不进去。
   现在认 `MOVIE/SERIES/SEASON/EPISODE`；带 `parent_id` 时统一按浏览集合返回，不再被调用方的
   `MOVIE`/`MOVIE,SERIES` 扁平过滤挡住。前端 `LibraryPage` 的目录下钻对所有板块开放（合集除外）。
2. **孤儿 FOLDER 自动清理**（同提交）：对账比对 `FOLDER.identity_key` 指向的目录是否存在，不存在就软删；
   库根 `is_available=0` 或路径没挂载时整根跳过。
3. **取消特摄剧板块**（`20fbb5e6`）：见 §2。已把 `假面骑士ZZZ (2025)` 迁到 `电视剧/国外/2020s`
   （115 + 镜像 + 库条目），`特摄剧` 库已删。
4. **REMUX 单独成库**（`9fbfa12b` + `36293df6`）：目录 `媒体目录/REMUX/<类型>/<地区>/<年代>/<片名>`，
   **库显示名「影视原盘」**（目录名保持 ASCII，归档侧按它过滤）。
   - REMUX 是来源属性不是分辨率：真实库里 7546 个 REMUX 文件有 3406 个是 1080p。
   - **分辨率不进目录**（用户明确）：清晰度只在文件名里，1080p/2160p remux 并排在同一片名目录。
   - 整包判定：包里只要有一个视频不带 `REMUX`，整包走普通板。
5. **国产短剧单独成库**（`d1a8beec` + `cbde11c0`）：`待整理/国产短剧/` 是手动归板入口，
   里面每个条目强制进该板；落 `国产短剧/<年代>/<片名 (年份)>/`，无地区层。
   **镜像会改写文件名**成 `Season 01/<片名> S01E01.strm` + 分集 NFO（115 上保留 `第01集.mp4` 原名），
   否则扫描器认不出集号、整包 0 条目。
6. 文档：`archive-pipeline.md`、`AUTO-LIBRARY.md` 已按以上全部更新。

## 4. 当前数据状态（2026-10-23 收盘）

库里 10 个：电影 / 电视剧 / 动画 / 综艺 / 纪录片 / 演唱会 / 晚会 / **影视原盘** / **国产短剧** / 合集。

| 库 | 条目 |
| --- | --- |
| 电影 | MOVIE 5（全在 国外） |
| 电视剧 | SERIES 19 · SEASON 20 · EPISODE 365 |
| 动画 | SERIES 139 · SEASON 159 · EPISODE 3085 · MOVIE 1 |
| 纪录片 | SERIES 2 · SEASON 2 · EPISODE 3 · MOVIE 1 |
| 综艺 | SERIES 1 · SEASON 1 · EPISODE 1 |
| 演唱会 | MOVIE 1 |
| 晚会 | MOVIE 1（200KB 合成样本） |
| 影视原盘 | MOVIE 1 · SERIES 1 · SEASON 1 · EPISODE 1（300KB/250KB 合成样本） |
| 国产短剧 | SERIES 2 · SEASON 2 · EPISODE 5 · MOVIE 1（150/120/80KB 合成样本） |
| 合集 | 0（内置全局合集库，不是自动入库建的，删了会自己回来） |

`待整理`、`转存目录` 为空；`失败目录` 里有几条 10-18/10-20 的旧桶和本轮两次 TMDB 瞬断的记录。

## 5. 没做完 / 下一步

1. **视觉验收（唯一卡在外部资源的项）**：按 `docs/handoff/VISUAL-CHECK-2026-10-23.md` 在浏览器里过一遍。
   访问 http://192.168.8.8:8099 ，先 Ctrl+F5。清单包含 10 个库的预期项，重点是电影库根层只有
   「国外」文件夹、影视原盘/国产短剧能一路点到条目。**本轮所有 UI 改动只验到接口层，没有一次真人眼验。**
2. **TG 频道监听 v2**（契约 `docs/handoff/CHANNEL-MONITORING.md`）：代取频道 → 设置 schema →
   每频道游标 + 判定矩阵。工作量最大，**用户明确要求放最后**。
3. 明确**不做**：真实库分类对齐。用户 2026-10-23 说"真实库分得细正是要改的原因"，Lux 只按自己的规则走；
   真实库继续只读（`影视库/媒体库` 与 `影视库/真实库` 都不许动）。
4. 可选微调：`影视原盘` 的中文名是 `auto_library::REMUX_BOARD_TITLE` 一行常量，要改直接改它。

## 6. 踩坑（按今天踩到的顺序）

1. **单文件条目的洗版看的是目标文件路径（含文件名）**，不是标题目录：只有同名且更大才洗；
   换个 `2160p` 名字再入库是"两个版本并排共存"。验证 wash 要用同名改大小（1MB→3MB 足够）。
2. **短剧镜像必须改集号**：`第01集.mp4` 原样写进镜像 → 扫描器认不出 → 整包 0 条目（实测过）。
   现在 `write_short_drama` 写 `Season 01/<片名> S01E0N.strm` + `<episodedetails>` NFO。
3. **取消一个板块要三件事一起做**：加 `RETIRED_BOARDS`、清镜像目录、删库
   （`DELETE /api/v1/admin/libraries/{id}`），否则对账会把库建回来。
4. **TMDB 偶发 `error sending request`**（不是 429）：同一批里 TV 查询成功、Movie 失败，
   同一个 URL 用 curl 又是 200。别怀疑代码——把条目从 `失败目录` 捞回 `待整理` 重跑即可。
5. **TMDB zh-CN 类型名要按接口值匹配**：genre 99 是「纪录」不是「纪录片」；没有「综艺」，
   中文综艺是「真人秀」/「脱口秀」。
6. **中文标题 + 点号**：`parse_media_name` 对 CJK 默认不切点，`中文名.S01E01…` 会整串粘住；
   现在"读到季集号"或"带发布标签能补年份"才用切分读法。
7. **单文件条目不能当目录包写镜像**：否则生成 `<文件>.mp4/` 假目录。
8. **挪走片名目录后必须删旧镜像目录**，不删就新旧并存、旧 strm 指向不存在的 115 路径。
9. **115 上 `mv` 是逐条 API 调用**：139 个目录好几分钟，用清单文件逐条记录，别 `mv *`。
10. **改库 kind 没有接口**：要删库重建（对账只在 `organized` 非空或启动时跑）。
11. `tests/admin_health.rs` 在干净 HEAD 也失败（`maxConnections` 期望 8、实际 24），环境相关，别当回归。

## 7. 常用验收命令

```bash
B=http://127.0.0.1:8099
curl -s -c /tmp/c.jar -X POST $B/api/v1/auth/login -H 'content-type: application/json' \
  -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)

# 库 / 条目（parentId=root 就是目录浏览的第一层）
curl -s -b /tmp/c.jar $B/api/v1/libraries
curl -s -b /tmp/c.jar "$B/api/v1/libraries/<lib_id>/items?pageSize=30&parentId=root"

# 手动跑归档（默认 dry_run=true）
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" "$B/api/v1/admin/archive/run?dry_run=false"
# 重扫某库
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" "$B/api/v1/admin/libraries/<lib_id>/scan"

# 单测（全量约 4.5 分钟，archive 子集约 2 分钟）
docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home -v /tmp/lux-u11:/src -v /vol5/1000/build/target:/src/target -w /src \
  rust:1.94-test bash -c "cargo test --locked --lib"
```

数据库：`/vol5/1000/build/archive-test-config/lux.db`；115 测试库：
`/vol5/1000/docker/clouddrive2/CloudNAS/115open/测试库`（容器内 `/testlib`）；
本地镜像根：`/vol5/1000/docker/lux-library`（容器内 `/library`）。

## 8. 交接语境

- §3 的六条都是**用户在对话里明确拍板**的，包括：特摄取消、REMUX 单独成库且用 `REMUX/` 目录、
  库名用中文「影视原盘」、分辨率不进目录、国产短剧单独成库且靠 `待整理/国产短剧/` 归板、
  真实库不对齐。不要当成我的推断翻案。
- 实现细节（镜像改集号、整包判定、`RETIRED_BOARDS`、`library_name_for_board`）是**我选的方案**，
  用户只定了行为目标；要改实现不必先问。
- 本轮**没有执行任何视觉验收**；所有"界面正常"的说法都只到接口层。
