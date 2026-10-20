from pathlib import Path
p = Path("/tmp/lux-u11/sidecar/telegram/tgwatch/health.py")
s = p.read_text(encoding="utf-8")
old = '''        def do_GET(self) -> None:  # noqa: N802 - http.server API
            parsed = urlparse(self.path)
            if parsed.path != "/healthz":'''
new = '''        def do_GET(self) -> None:  # noqa: N802 - http.server API
            parsed = urlparse(self.path)
            if parsed.path == "/channels":
                # 设置页「刷新频道列表」：把账号里的频道交给 Lux，由它渲染勾选界面。
                try:
                    channels = asyncio.run(runner.list_channels(config))
                except Exception as error:  # noqa: BLE001 - 探针式接口不抛栈
                    self._send(503, {"ok": False, "channels": [], "detail": str(error)})
                    return
                self._send(200, {"ok": True, "channels": channels})
                return
            if parsed.path != "/healthz":'''
assert s.count(old) == 1
s = s.replace(old, new)
p.write_text(s, encoding="utf-8")
print("health patched")