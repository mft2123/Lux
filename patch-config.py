from pathlib import Path
p = Path("/tmp/lux-u11/sidecar/telegram/tgwatch/config.py")
s = p.read_text(encoding="utf-8")
old = '''    problems: list[str] = []
    if not config.channels:
        problems.append("telegram.listen_channels 为空")
'''
new = '''    problems: list[str] = []
    # 频道为空不是硬错误：设置页要先能列出账号里的频道，才有了勾选的前提。
    # 空配置下侧车空转（/healthz、/channels 仍可用），由 runtime_warnings 提示。
'''
assert s.count(old) == 1
s = s.replace(old, new)
p.write_text(s, encoding="utf-8")
print("config patched")