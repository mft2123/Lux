# Lux 交接（2026-10-21 收尾，给下一次会话）

> **先读这个文件，再读两份契约**：
> - `docs/archive-pipeline.md` —— 归档流水线的行为规范（代码按它执行，改算法先改它）
> - `docs/handoff/AUTO-LIBRARY.md` —— 自动入库契约
> - `docs/handoff/CHANNEL-MONITORING.md` —— TG 频道监听 v2（还没做）
>
> 上一轮（2026-10-20：U11 115 转存、5 个真 bug、设置页、主题、TG 侧车、封面）的全文在
> `docs/handoff/ARCHIVE-2026-10-20.md`，需要背景时再翻。
>
> 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（主 checkout 在 `feat/telegram-ingest`）。
> 本轮工作区：worktree `/tmp/lux-u11`，分支 `fix/u11-e2e`。

## 1. 当前基线

| 项 | 值 |
| --- | --- |
| 代码提交 | `b02cbc9f` `feat(archive): one library per type board`（本文件另有一次 docs 提交，在它之后） |
| 镜像 | `lux:phase-a` = `0514bbafeb6a`（已部署，容器在跑） |
| 测试容器 | `lux-testlib`（8099→8097，admin/`Test@12345a`，healthy） |
| 115 侧车 | `lux-transfer115`（8098，healthy） |
| TG 侧车（测试） | `lux-telegram-test`（8091，24 个频道） |
| 测试配置 | `/vol5/1000/build/archive-test-config` |
| 构建/测试 | docker `rust:1.94-test`，target 共享在 `/vol5/1000/build/target` |

**容器需要 TMDB key**（否则归档/回填全失败）：

```bash
KEY=$(python3 -c "import json;print(json.load(open('/vol5/1000/docker/chillposter/config/settings.json'))['tmdb_key'])")
# 重建容器时加：-e LUX_TMDB_API_KEY="$KEY"
```

## 2. 本轮（10-21）做了什么

### 2.1 目录层：年份 → 年代

`媒体目录/<类型>/<国内|国外>/<年代>/<片名 (年份)>/`。`2024` → `2020s`，无年份 → `未知年代`，
与用户既有 `影视库/媒体目录` 一致。库内查找同时认「年份」和「年代」，老目录还能解析。

### 2.2 自动入库（新增，`src/application/archive/auto_library.rs`）

**一个类型一个库**，和电影、电视剧完全一样：`电影` / `电视剧` / `动画` / `综艺` / `晚会` / `演唱会` …
地区（国内/国外）和年代是库**里面**的目录层，**永不拆库**。

- 库类型：含「电影」→ `MOVIE`；动画板块 → `MIXED`（既有动画电影也有番剧，按文件逐个判定）；其余 `SERIES`
- `动漫` 是动画板块的别名，合并进「动画」库（第二个根），不新建库
- 空目录（无 strm/nfo）不建库
- 幂等键 = 库根路径；已存在只补缺的根
- 建库后自动入队一次全量扫描（复用 `ScanJobService`，不另起通道）
- 触发三个：① 归档 run 结束（`organized` 非空）② 启动对账 ③ 每 600s 兜底对账
- 接线：`AppState::register_archive_auto_library()`（`main.rs` 在 `start_poller()` 前调用），
  `start_poller()` 签名未变

### 2.3 动画 / 动漫最终口径

**合并成一个板块「动画」**，只按地区分国内/国外，且**地区只是库内目录**。
归类器 `category_label()` 只输出 `动画`（不再按产地分 `动漫`）。

### 2.4 测试库数据迁移（115 侧，只动 `测试库/媒体目录`）

1. `动画/<年代>/` → `动画/国内|国外/<年代>/`（141 条：国内 112、国外 29）
2. 存量年份目录 → 年代目录（电影 5 个、电视剧 8 个年份桶，22 条片）
3. `动漫/*` 合并回 `动画/*`（29 条），`动漫` 空目录已删
4. 回填镜像：`POST /api/v1/admin/archive/metadata?dry_run=false`
5. 建库来回建过几次，最后只留三个库（旧库 DELETE 后由对账重建）

## 3. 当前数据状态（实测）

115 `测试库/媒体目录`：

```
电影/国外/{1980s,2000s,2010s}          （电影只有国外，没有国内）
电视剧/{国内,国外}/{1990s,2010s,2020s}
动画/{国内/{2010s,2020s}, 国外/2020s}
```

本地镜像 `/vol5/1000/docker/lux-library`：结构同上（去掉 `媒体目录/` 这层），每部片是
`.strm` + NFO + 海报。

| 库 | kind | 根 | 条目 |
| --- | --- | --- | --- |
| 动画 | MIXED | `/library/动画`（国内、国外都在库内） | SERIES 129 · MOVIE 1 · SEASON 140 · EPISODE 2913 · FOLDER 3 |
| 电影 | MOVIE | `/library/电影` | MOVIE 5 · FOLDER 9 |
| 电视剧 | SERIES | `/library/电视剧` | SERIES 18 · SEASON 19 · EPISODE 364 |
| 合集 | MIXED | 无根、0 条目 | 早先遗留的空库 |

扫描任务：3 条全部 `COMPLETED`。web 库列表：`['动画', '电影', '电视剧']`。

## 4. 已定的事（别翻案）

1. **一类型一库**：`动画`、`电影`、`电视剧` 各一个库。10-21 当天先按产地拆过两个库、又按地区拆过两个库，
   两次都被用户纠正——**不要再按地区/产地拆库**。
2. **国内/国外、年代是库内目录层**，不是库。
3. **动画与动漫是同一个板块**，只分国内/国外。
4. **晚会不分地区**（`晚会/<年代>/…`），**演唱会分地区**（`演唱会/<国内|国外>/<年代>/…`），各一个库。
5. **只动测试库**：真实库 `影视库/媒体目录` 全程只读。真实库等调试完成后**分批复制**到测试库再跑。
6. 年代段与真实库一致（真实库是 `<类型>/<地区>/<年代>/<片名>`）。

## 5. 未完成（按优先级）

1. **电影库只有 5/16 部识别成 MOVIE，其余 9 条是 FOLDER**（连 `1980s`、`国外` 这种目录也被扫成条目）。
   既有问题，不是本轮引入的，但真实库接进来会放大——建议批量复制前先修。
2. **`合集` 空库**（MIXED、无根、0 条目，web 不显示）：删或留着待用户定。
3. **失败原因文件 `原因.txt` 乱码**：内容其实是合法 UTF-8，Windows 记事本按 GBK 打开才乱。
   修法：写入时加 UTF-8 BOM（`EF BB BF`），改 `src/application/archive.rs` 写原因文件处。
4. **真实库分批复制方案**：真实库一级目录是 `剧集/动漫/电影/综艺/纪录片/晚会/演唱会/特摄剧/…`，
   与 Lux 写的 `电视剧/动画` 不同；复制时要么改名为 Lux 口径，要么依赖别名（`动漫` 已支持，`剧集` 尚未）。
5. **TG 频道监听 v2 的 Lux 侧**（契约 `CHANNEL-MONITORING.md`）：代取频道 → 设置 schema → 每频道游标 +
   判定矩阵。`manual` 是派生值（其余四个全 false），不是独立开关。
6. **封面/主题的视觉验收**：本机无浏览器，10-20 那批 UI 改动只验到编译+接口，没人眼看过。
7. 回填生成的镜像目录名保留了 115 侧的 `{tmdb-123}` 后缀（归档 run 生成的不带），只影响观感。
8. TMDB `{tmdb-ID}` 提示可能跨命名空间误命中（同一 id 在 movie/tv 下都存在），目前靠人工核对。

## 6. 踩坑（本轮新增，必看）

1. **年份→年代迁移要合并内容**：把 `1989/` 目录整个塞进 `1980s/` 会变成 `1980s/1989/<片>`，
   回填时报 `无法从目录名解析`。正确做法是把年份目录的**内容**挪进年代目录，再删空目录。
2. **`"动漫".contains("动画")` 是 false**：任何按分类名的启发式（回填的 series 判定、自动入库）都要
   两个都写，否则 `动漫` 整块被静默跳过。
3. **动了 `媒体目录` 就必须重建镜像**：strm 内容里带着 115 侧的完整路径，只挪目录不改 strm 就是死链。
   标准动作：挪 115 → 删对应镜像目录 → `POST /admin/archive/metadata?dry_run=false` → 重扫。
4. **改库的类型要删库重建**：`SERIES` → `MIXED` 这种没有接口改，DELETE 库再让对账重建即可
   （条目会重新扫回来）。
5. **回填/归档需要 TMDB key**：容器默认没有，重建时加 `-e LUX_TMDB_API_KEY`。
6. **`tests/admin_health.rs` 在干净 HEAD 也失败**（`maxConnections` 期望 8、实际 24），环境相关，别当回归。
7. `docker build` 成功不代表改动进包：看**镜像哈希是否变化**，以及 `assets/index-*.js` 哈希。
8. 别用 `git branch -f … HEAD`（主 checkout 的 HEAD 会把主干指回旧提交），要给显式 SHA。

## 7. 常用验收命令

```bash
# 登录 + CSRF
curl -s -c /tmp/c.jar -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H 'content-type: application/json' -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)

# 归档状态 / 回填镜像 / 重启扫描
curl -s -b /tmp/c.jar http://127.0.0.1:8099/api/v1/admin/archive/status
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" \
  "http://127.0.0.1:8099/api/v1/admin/archive/metadata?dry_run=false"

# 自动入库日志
docker logs lux-testlib 2>&1 | grep -E "archive auto library|archive pipeline run completed"

# 库 / 根 / 扫描 / 条目
python3 - <<'PY'
import sqlite3
c = sqlite3.connect('/vol5/1000/build/archive-test-config/lux.db')
print(list(c.execute('select name,kind from libraries order by name')))
print([r[0] for r in c.execute('select display_path from library_roots order by display_path')])
print(list(c.execute('select status,count(*) from scan_jobs group by status')))
print(list(c.execute("select l.name,m.item_type,count(*) from media_items m join libraries l on l.id=m.library_id group by 1,2")))
PY

# 单测（只跑归档相关，约 2 分钟）
docker run --rm -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.1 -e RUSTUP_DIST_SERVER=https://rsproxy.cn \
  -v /vol5/1000/build/cargo-home:/cargo-home -v /tmp/lux-u11:/src -v /vol5/1000/build/target:/src/target -w /src \
  rust:1.94-test bash -c "cargo test --locked --lib archive"

# 重建镜像（约 5 分钟，web 层有缓存）
cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .
```

`/tmp/recreate3.sh` 是重建 `lux-testlib` 的脚本（含 TMDB key、全部挂载与端口）。

## 8. 建议的下一步顺序

1. 修电影库的 FOLDER 噪音（影响真实库接入体验）
2. `原因.txt` 加 BOM（单点、10 分钟）
3. 定真实库分批复制方案（改名的映射表 + 每批复制后的回填/重扫流程）
4. TG 频道监听 v2 的 Lux 侧（契约已写好，工作量最大）
5. 找一台有浏览器的机器做封面/主题的视觉验收
