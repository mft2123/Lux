# transfer115 —— 115 分享链接转存侧车

Lux 链接队列里只有 `needs_115_client` 这一个状态是空的：Telegram 旁挂把 115 分享链接交给 Lux，
Lux 归档流水线能整理文件，但**没有任何东西真正登录 115 去转存**。这个侧车就是补这一段：

```
Telegram → Lux ingest → 链接队列/*.json
                            │  POST /transfer
                            ▼
                    transfer115 侧车 ── p115client + 网页 cookie ──> 115 网盘「转存目录」
                            │                                        （CD2 挂载可见）
                            ▼
                    归档流水线：转存目录 → 待整理 → 媒体目录 → 镜像
```

它只做一件事：**把 115 分享链接转存到 115 账号里的目标目录**。转存完成后文件出现在 CD2 挂载中，
后续整理、识别、洗版、镜像全部由 Lux 现有流水线完成，这里不碰。

## 契约（与 Lux 侧 `570ccb19` 对齐）

`POST /transfer`

```json
{
  "job_id": "1770000000000-abc",
  "url": "https://115.com/s/sw123abc?password=abcd",
  "title": "某剧 第二季",
  "archive_root": "/testlib",
  "target_dir": "转存目录",
  "cd2_prefix": "测试库"
}
```

响应（HTTP 200）：

```json
{"status": "transferred", "detail": "已转存 1 个条目", "saved": ["某剧 第二季"]}
```

| status | 含义 | Lux 侧行为 |
| --- | --- | --- |
| `transferred` | 已转存（或已提交、115 仍在处理） | 记账、移入 `已处理/` |
| `failed` | 分享失效/需要提取码/接口报错 | 记账 `failed`、移入 `已处理/` |
| `auth_required` | cookie 失效 | **不记账、不移文件**，作业留在队列，换 cookie 后下一轮重试 |
| `unsupported` | 不是 115 分享链接 | 记账 `unsupported` |

> magnet / ed2k 不会到这里：Lux 侧直接记 `unsupported`（115 云下载是后续切片）。

## 配置

侧车不接收 cookie，**从 Lux 的设置文件里读**（同一份 `archive-settings.json`）：

```json
{
  "transfer115": {
    "cookie": "UID=...; CID=...; SEID=...; KID=...",
    "target_dir": "转存目录",
    "sidecar_base": "http://transfer115:8098",
    "timeout_seconds": 120
  }
}
```

- `cookie`：115 网页登录后的整串 cookie（浏览器 F12 → Network → 任意请求 → Request Headers → Cookie）。
  设置页里打码显示，留空表示不修改。
- `target_dir`：相对归档根的目标目录，侧车会把它拼成 `<cd2_prefix>/<target_dir>` 再解析成 115 的 `cid`，
  目录不存在会自动创建。
- `sidecar_base`：空 = 关闭转存阶段（作业只记 `needs_115_client`）。

侧车自己的环境变量：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `TRANSFER115_CONFIG_DIR` | `/config` | 读 `archive-settings.json` 的目录 |
| `TRANSFER115_HOST` / `TRANSFER115_PORT` | `0.0.0.0` / `8098` | 监听地址 |
| `LUX_115_COOKIE` | 空 | 覆盖设置文件里的 cookie（本地调试用） |
| `TRANSFER115_CONFIRM_SECONDS` | `30` | 转存后等落盘的确认时间，0 = 不等待 |
| `TRANSFER115_POLL_INTERVAL` | `2` | 确认轮询间隔 |
| `TRANSFER115_IS_CHECK` | `0` | 传给 115 `share/receive` 的 `is_check`，0 = 真转存 |

## 权限（重要）

设置文件是 `0600`、属主 root。侧车以另一个 uid 运行，**必须**按 Lux 的既有约定给它读权限：

```yaml
services:
  transfer115:
    image: lux-transfer115:latest
    restart: unless-stopped
    environment:
      TRANSFER115_CONFIG_DIR: /config
    volumes:
      - /path/to/config:/config:ro
    group_add:
      - "${LUX_ARCHIVE_SETTINGS_GROUP:-10001}"
```

宿主机上把配置目录设成 `0750 root:<group>`、设置文件 `0640 root:<group>`，再把侧车容器的
`group_add` 指到同一个组（和 Telegram 旁挂同一套做法）。

## 运行

```sh
# 本地
python -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt
TRANSFER115_CONFIG_DIR=/path/to/config python -u main.py

# 镜像
docker build -t lux-transfer115:latest .
```

健康检查：

```sh
curl -s http://127.0.0.1:8098/healthz          # 进程在 + 有没有 cookie
curl -s 'http://127.0.0.1:8098/healthz?deep=1' # 真的拿 cookie 去 115 验一次登录
```

## 测试

离线用例（不联网、不碰真账号），在装有 p115client 的 Python 3.12 环境里：

```sh
python -m unittest discover -s tests -t .
```

覆盖：链接解析、cookie 读取/打码识别、目标目录解析与创建、多条目转存、`transferred`/`failed`/
`auth_required`/`unsupported` 四种状态、HTTP 往返与参数校验。

## 已知限制

- 只处理 115 分享链接；magnet/ed2k 由 Lux 侧直接记 `unsupported`。
- 同一个 115 会话串行转存（一次一个作业），避免并发请求互相打架。
- 分享需要提取码但链接里没带 → `failed`，detail 会说明原因。
- 大分享在 115 侧可能仍在处理；此时侧车返回 `transferred` 并在 detail 里注明「等待 115 完成」，
  文件落盘后由归档流水线照常接走。
- 日志只打 `job_id` 和 URL 的 host，**不打完整分享链接**（链接里可能带提取码）。
