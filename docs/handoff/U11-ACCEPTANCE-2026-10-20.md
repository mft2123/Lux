# U11 真实 cookie 端到端验收（2026-10-20）

环境：镜像 `lux:phase-a` = `5aaa2f1ef7fa`（基线 `ae2f9e4a` + 本轮修复），侧车 `lux-transfer115:latest`
（**独立容器**，8098，不进 chill），测试容器 `lux-testlib`（8099，重建后 healthy）。
NAS 可直连 `115.com` / `webapi.115.com` / `115cdn.com`（均 200）。

## 通过的关键路径

| 步骤 | 结果 |
| --- | --- |
| 设置读写 | `transfer115` 四项落盘（cookie 打码 230 字符、`sidecar_base`、`target_dir`、`timeout`），0600 |
| 登录态 | `GET /healthz?deep=1` → `logged_in`（真实 cookie） |
| 转存 | `status=transferred`，`状态.jsonl` 记 `transferred` /「已转存 1 个条目」 |
| 115 落盘 | `测试库/转存目录/Whistle (2026) {tmdb-1759321}/`（8.9 GiB 分享） |
| CD2 可见 | 挂载在 20s 内出现该目录 |
| 归档接管 | 流水线搬入 `待整理`，随后因「未配置 TMDB API Key」进 `失败目录/1792486275`（测试实例没配 key，非 U11 缺陷） |
| 失败分支 | 坏 cookie 侧车 → `auth_required`，作业**留在链接队列**，`状态.jsonl` 行数不变（6→6） |

## 本轮修掉的 3 个真 bug（都会挡住真实链接）

1. **Rust 只认 `115.com`**：`115cdn.com` 被判 `unsupported`，作业根本到不了侧车。现已与侧车
   `SHARE_HOSTS` 对齐（`115cdn/anxia/115.link/115.com.cn`）。
2. **分享密码粘着说明文字**：`?password=x3c1，请及时检查…` 会把整串当提取码发出去。现在只取前导 ASCII token。
3. **auth 判定失效**：`login_status()` 失效时返回**裸 `False`**（不是 dict、不抛异常），`check_login` 不认；
   而且 115 把「登录超时」放在 HTTP 200 的 body 里（`state:false, errno 990001`），侧车也不认。
   后果是作业被记 `failed` 并移出队列，而不是 `auth_required` 留队重试。两处都修了。

## 其它建立的事实

- `p115client==0.0.9.4` 不可复现：必须钉 `python-asynctools==0.1.4`（今天解析到的 0.2.2 缺 `to_list`，镜像启动即崩）。
- `Dockerfile` 支持 `PIP_INDEX_URL`：部署网段到 `pypi.org` 不通，默认清华源即可构建。
- 基础镜像 `python:3.12-slim` 用 `docker.m.daocloud.io/library/python:3.12-slim` 拉取（`registry-1.docker.io` 直连 EOF）。
- 测试：Rust `cargo test --lib archive` **83 passed**；侧车 **27 passed**（新增 6 条，覆盖 115 真实 payload 形状与 auth 分支）。

## 仍未验证 / 边界

- magnet / ed2k 仍记 `unsupported`（115 云下载是后续切片）。
- 侧车镜像未推 registry；当前由本机构建使用。
- 测试实例没配 TMDB key，所以「媒体目录 + 镜像」最后一段没在本次验收里跑通（属既有环境缺项）。