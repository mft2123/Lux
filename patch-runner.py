from pathlib import Path

r = Path("/tmp/lux-u11/sidecar/telegram/tgwatch/runner.py")
s = r.read_text(encoding="utf-8")

old = '''    if not config.channels:
        raise RunnerError("listen_channels 为空：没有可监听的频道")'''
new = '''    if not config.channels:
        # 空配置不再拒绝启动：设置页要先能列出账号里的频道，才有了勾选的前提。
        # 这里空转（/channels 仍可用），勾选并重启后开始投递。
        log.warning("listen_channels 为空：侧车空转，等勾选频道后重启")
        try:
            await asyncio.Event().wait()
        finally:
            return'''
assert s.count(old) == 1, s.count(old)
s = s.replace(old, new)

anchor = '''def _new_client(config: config_mod.SidecarConfig):'''
add = '''async def list_channels(
    config: config_mod.SidecarConfig,
    *,
    client_factory=None,
    timeout: float = CHANNEL_LIST_TIMEOUT,
) -> list[dict]:
    """列出账号里的全部频道（设置页「刷新频道列表」用）。

    只取 `is_channel` 的会话（不含普通群），返回稳定键与展示字段。
    `getDialogs` 在几千个会话的账号上会慢到分钟级，所以调用方要缓存。
    """
    if not config.session_path.exists():
        raise RunnerError(f"session 不存在（{config.session_path}）：先完成首次登录")
    factory = client_factory or _new_client
    client = factory(config)
    try:
        with session_errors(config):
            await asyncio.wait_for(client.connect(), timeout)
            dialogs = await asyncio.wait_for(client.get_dialogs(), timeout)
    except asyncio.TimeoutError as error:
        raise RunnerError(f"读取频道超时（{timeout:g}s）：检查容器网络/代理") from error
    finally:
        with contextlib.suppress(Exception):
            await client.disconnect()
    channels = []
    for dialog in dialogs:
        entity = getattr(dialog, "entity", None)
        if not getattr(dialog, "is_channel", False) or entity is None:
            continue
        username = (getattr(entity, "username", "") or "").lstrip("@")
        raw_id = getattr(entity, "id", None) or getattr(dialog, "id", None)
        channels.append(
            {
                "telegramId": str(raw_id or ""),
                "username": username,
                "title": getattr(dialog, "title", "") or username or "未命名频道",
            }
        )
    return channels


'''
assert s.count(anchor) == 1
s = s.replace(anchor, add + anchor)

# 常量
old_const = "DEEP_CHECK_TIMEOUT"
i = s.index(old_const)
line_end = s.index("\n", i)
s = s[: line_end + 1] + "#: 频道清单要让用户等得下去，但也不能无限等。\nCHANNEL_LIST_TIMEOUT = 60.0\n" + s[line_end + 1 :]
r.write_text(s, encoding="utf-8")
print("runner patched")