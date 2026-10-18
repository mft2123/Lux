"""Lux Telegram 旁挂：频道监听 → 链接提取 → 五模式过滤 → 投递。

模块边界（见 docs/handoff/module-telethon.md）：
- 只读 `/config/archive-settings.json` 的 `telegram` 段，自己不改 Lux 的任何文件
- 纯逻辑层（links / filters / store / sink / config）不依赖 telethon，可在没有网络的机器上跑单测
- 只有 runner.py 会 import telethon
"""

__all__ = ["config", "filters", "links", "logsafe", "sink", "store"]
