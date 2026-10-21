# Lux 交接（2026-10-20 收尾）

> 给下一个对话：先读本文件，再读 `HANDOFF.md`（历史全貌）与 `CHANNEL-MONITORING.md`、`AUTO-LIBRARY.md`（两份新契约）。
> 仓库：`qiangli@192.168.8.8:/vol5/1000/build/lux`（**主 checkout 在 `feat/telegram-ingest`，不是主干**）。
> 本轮所有工作都在 worktree `/tmp/lux-u11`（分支 `fix/u11-e2e`），已 fast-forward 到主干。

## 1. 当前基线

| 项 | 值 |
| --- | --- |
| 主干 `fork/phase-a` / `integration/round2` | `b970604f` |
| 镜像 `lux:phase-a` | `0c20b6520f77` |
| 测试容器 | `lux-testlib`（8099→8097，admin/`Test@12345a`，healthy） |
| 115 侧车 | `lux-transfer115`（8098，healthy，`logged_in`） |
| TG 侧车（测试用） | `lux-telegram-test`（8091，`logged_in`，24 个频道） |
| 测试配置 | `/vol5/1000/build/archive-test-config`（含真实 115 cookie 与 chill 的 TG 凭证） |
| 镜像 lux: 前端产物 | `assets/index-*.js`，改动后哈希必变（可用它判断是否进包） |

## 2. 本轮完成（都有实测证据）

### 2.1 U11 115 转存端到端（真实 cookie）
- 链路全通：`ingest → 侧车 share_receive → 115 转存目录 → CD2 可见 → 待整理`。
- 实测落盘 `测试库/转存目录/Whistle (2026) {tmdb-1759321}/`。
- 失败分支：坏 cookie → `auth_required`，作业留队、`状态.jsonl` 行数不变。

### 2.2 修掉的 5 个真 bug
1. Rust 只认 `115.com`，`115cdn.com` 被判 unsupported（已与侧车 `SHARE_HOSTS` 对齐）。
2. 分享密码粘连说明文字（`?password=x3c1，请及时检查…`）被整串当提取码。
3. `login_status()` 失效时返回裸 `False`；115 又把登录失效放在 HTTP 200 的 body（`errno 990001`）——两处都不认，导致作业被记 `failed` 而不是留队。
4. 侧车 `_is_dir_of` 只认 `is_dir` 键，而 115 的真实列表用 `cid`/`fid` 区分目录与文件。
5. `p115client==0.0.9.4` 不可复现：必须钉 `python-asynctools==0.1.4`（0.2.2 缺 `to_list`，镜像启动即崩）。

### 2.3 设置页
- 归档设置拆成 6 个区、**各自保存**；115 区加「测试连接」（服务端代测 `healthz?deep=1`）。
- 全局「服务器设置」拆出「媒体库显示」，同样各自保存；表单只在首次加载填充，不再覆盖未保存编辑。

### 2.4 主题
- 顶栏加日/月切换（原先只在「账户→外观」）。
- 主按钮跟随强调色（silver 保持原样）。
- 浅色主题可读性：后台标题、图标按钮、输入框、状态框回退到主题变量。
- 下拉可读性：`.lux-select-option` 改用 `--lux-text`；原生 `<select>` 的 `option` 按主题显式定色。

### 2.5 TG 侧车（频道清单的地基）
- `GET /channels`：`iter_dialogs` 取账号频道（只留 `is_channel`），60s 超时、失败断开重连。
- `listen_channels` 为空不再 `exit 2`，改为空转 + WARNING（否则永远没机会列频道）。
- `runtime_problems` 不再把「频道为空」当不健康。
- 实测：`/healthz` → configured；`?deep=1` → logged_in；`/channels` → 24 个频道。
- 单测 `tests/test_channels.py` 2 条通过。

### 2.6 封面
- 新增 `GeneratedCover`（本地绘制，无网络）：电影酒红 / 剧集藏蓝 / 动画紫，片名 + 年份 + 类型角标。
- 已接进 `MediaPlaceholder`（无图条目）。
- 电影卡片 `<img>` 加 `onError` 回退到生成封面（电影有 imageTags 但图片 404，导致空白）。

## 3. 未完成（按优先级）

1. **自动入库**（契约已写：`AUTO-LIBRARY.md`，尚未实现）
   - 规则：`媒体目录/<分类>` → 同名库（电影 MOVIE、电视剧/动画/晚会/演唱会 SERIES），幂等键=库根路径，空目录不建库。
   - 触发：归档 run 结束 + 启动对账；建库后自动全量扫描。
   - **前置**：`lux-library` 里没有 `动画` 目录 → 先查归档分类→镜像生成为何漏了动画。
2. **TG 频道监听 v2 的 Lux 侧**（契约：`CHANNEL-MONITORING.md`）
   - `POST /admin/archive/telegram/channels` 代取 → 设置 schema `channels[{id,telegramId,username,title,enabled,modes}]` → 页面勾选 → 每频道 `lastMessageId` 游标 + 判定矩阵。
   - 注意：`manual` 是派生值（其余四个全 false），不是独立开关。
3. **失败原因文件乱码**：`失败目录/<ts>/原因.txt` 实际是**合法 UTF-8**，Windows 默认 GBK 编辑器打开才乱码。
   修法：写入时加 UTF-8 BOM（`EF BB BF`），在 `src/application/archive.rs` 写原因文件处加前缀即可。
4. **封面/主题的视觉验收**：本机无浏览器，以上 UI 改动只验到「编译通过 + 产物含新代码 + 接口行为」，**没有真眼看过**。

## 4. 踩坑（本轮新增）

1. `docker build` 成功的旧哈希可能表示**改动没进 bundle**（模块未被引用会被 tree-shake）。判断依据应是镜像哈希是否变化。
2. 权限：测试设置文件是 0600 root，TG 侧车以 uid 10001 跑会读不到；测试时用 `--user 0` 或 `group_add`。
3. TG 侧车 `sink=http` 必须要有 Lux 管理 API key（可从 `POST /api/v1/admin/api-key/rotate` 取），否则 exit 2。
4. 用 chill 的 session 做真机测试：复制 `/vol5/1000/docker/chillposter/config/telegram_user.session` 到测试目录并重命名 `lux-telegram.session`，**不要动原件**。
5. `docker inspect ... --format '{{...}}'` 在 Windows 的 ssh 传参里会被引号吃掉，写脚本文件执行更稳。
6. 别再用 `git branch -f ... HEAD`（在 `-C 主仓库` 下 HEAD 指主 checkout，会把主干指回旧提交）；要给显式 SHA。

## 5. 常用验收命令

```bash
# 测试容器登录
curl -s -c /tmp/c.jar -X POST http://127.0.0.1:8099/api/v1/auth/login \
  -H 'content-type: application/json' -d '{"username":"admin","password":"Test@12345a"}'
csrf=$(awk '$6=="lux_csrf" {print $7}' /tmp/c.jar)

# 115 / TG 探活
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" \
  http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe
curl -s -b /tmp/c.jar -X POST -H "X-CSRF-Token: $csrf" \
  http://127.0.0.1:8099/api/v1/admin/archive/telegram/probe
curl -s http://127.0.0.1:8091/channels

# 构建（总控/交接者）
cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .
```
---

# 2026-10-21 本轮：年代段 + 自动入库 + 动画分库

## 6. 改了什么

1. **归档目标改为年代**：`<类型>/<国内|国外>/<年代>/<片名 (年份)>/`（`2024` → `2020s`，无年份 → `未知年代`），
   对齐用户既有 `影视库/媒体目录` 结构。`is_library_year_segment` 同时认「年份」和「年代」，
   老目录（`电影/国外/1989`）仍能被库内查找/洗版逻辑解析。
2. **自动入库**（`src/application/archive/auto_library.rs`，契约 `AUTO-LIBRARY.md`）：
   镜像一级目录 → 一个库；**动画拆「国内动画 / 国外动画」两个库**；空板（无 strm/nfo）不建库；
   幂等键 = 库根路径；建库后入队一次全量扫描。
   触发三个：归档 run 结束（organized 非空）、启动对账、每 600s 兜底对账。
3. **接线**：`AppState::register_archive_auto_library()`（`main.rs` 在 `start_poller()` 前调用）
   把 database / libraries / scan_jobs 交给 poller，`start_poller()` 签名不变。

## 7. 测试库数据迁移（115 侧，已做）

- `测试库/媒体目录/动画/<年代>/` → `动画/<国内|国外>/<年代>/`，共 **141 条：国内 112、国外 29**。
  地区取自 `影视库/媒体目录/动漫/国产动漫|日本动漫` 的同名条目（测试库那批动画是从真实库复制出来的、
  复制时丢了地区层，所以原始数据里根本没有地区信息）。
- 2 条目录名里的 `{tmdb-ID}` 是错的（`46711` → CBC News、`319496` → 生命的极限）：已从目录名里去掉 id，
  让 TMDB 搜索重新解析（长江7号爱地球 正确命中 46711 电影条目）。
- 回填镜像：`POST /api/v1/admin/archive/metadata?dry_run=false` → `created 6272, failed 0`，6m41s。
  注意：测试容器默认没有 TMDB key，重建容器时加了 `-e LUX_TMDB_API_KEY=$(chillposter settings.json:tmdb_key)`。

## 8. 实测证据（镜像 `lux:phase-a` = `66a3c85fdcbf`）

| 项 | 结果 |
| --- | --- |
| 启动对账 | `created:["国外动画"] existing:3 scans:["国外动画"] failed:[]` |
| 幂等（再重启） | `found nothing to create, existing:5`；库/根数量不变 |
| 触发点①（run 结束） | 放「周杰伦地表最强演唱会 2019」进 `待整理` → `organized:1` → 立即 `created:["演唱会"]` 并入队扫描 |
| 空目录 | 镜像里建空目录「测试空板」→ 没有建库（验证后删除） |
| 最终库 | 剧集 359 / 国内动画 3053 / 国外动画 123 / 电影 16，扫描全 COMPLETED、error 为空 |
| 镜像完整性 | 抽样 `X特遣队的奇幻之旅`：115 侧 10 文件 → 镜像 10 strm + NFO + 海报 |
| 单测 | `cargo test --lib` 1094 passed |

`tests/admin_health.rs` 在**干净 HEAD `702a2ead` 上同样失败**（`maxConnections` 期望 8、实际 24），
是环境相关的既有失败，与本轮无关。

## 9. 已知 / 待办

1. **存量年份目录没迁**：`媒体目录/电影/国外/1989`、`电视剧/国内/2024` 这些还是年份层，新归档写年代层，
   两套并存（功能上都能扫）。要统一得单独做一次迁移 + 重扫，用户尚未点头。
2. 回填生成的镜像目录名保留了 115 侧的 `{tmdb-123}` 后缀（归档 run 生成的不带），只影响观感。
3. TMDB `{tmdb-ID}` 提示可能跨命名空间误命中（同一 id 在 movie/tv 下都存在），目前靠人工核对。
4. 第 3 节的老待办仍在：`原因.txt` 加 BOM、TG 频道监听 v2 的 Lux 侧、封面/主题的真眼验收。

## 10. 本轮验证命令

```bash
docker logs lux-testlib 2>&1 | grep -E "archive auto library|archive pipeline run completed"
# 库 / 根 / 扫描状态
python3 - <<'PY'
import sqlite3
c = sqlite3.connect('/vol5/1000/build/archive-test-config/lux.db')
print([r for r in c.execute('select name,kind from libraries order by name')])
print([r[0] for r in c.execute('select display_path from library_roots order by display_path')])
print(list(c.execute('select status,count(*) from scan_jobs group by status')))
PY
# 重建镜像
cd /tmp/lux-u11 && DOCKER_BUILDKIT=1 docker build -f Dockerfile.custom -t lux:phase-a .
```
