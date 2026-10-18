#!/usr/bin/env python3
"""Lux Telegram 旁挂入口。

    python main.py run               # 首次运行即登录（验证码提示在日志里），之后常驻监听
    python main.py status            # 配置（打码）+ 状态库统计
    python main.py check             # 只校验配置，不起网络
    python main.py held              # 看被挂起的条目（手动模式 / 洗版判定未知）
    python main.py replay --all      # 管理员触发：把挂起/失败/pending 的条目推给 Lux

退出码：0 正常，1 check 有硬错误，2 配置或登录前置条件缺失。
"""

from __future__ import annotations

import argparse
import asyncio
import dataclasses
import json
import logging
import os
import signal
import sys
from pathlib import Path

if __package__ in (None, ""):  # 允许 `python /app/main.py` 直接跑
    sys.path.insert(0, str(Path(__file__).resolve().parent))

from tgwatch import config as config_mod
from tgwatch import filters, logsafe, runner, sink as sink_mod, store as store_mod

log = logging.getLogger("tgwatch")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="lux-telegram", description="Lux Telegram 频道监听旁挂")
    parser.add_argument(
        "--settings",
        default=None,
        help="设置文件路径（默认 $LUX_ARCHIVE_SETTINGS 或 /config/archive-settings.json）",
    )
    parser.add_argument("--log-level", default=None, help="覆盖 LUX_TG_LOG_LEVEL")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("run", help="常驻监听（首次运行会走登录流程）")
    sub.add_parser("status", help="打印配置（打码）与状态库统计")
    sub.add_parser("check", help="校验配置，不联网")

    held = sub.add_parser("held", help="列出被挂起的条目")
    held.add_argument("--limit", type=int, default=50)
    held.add_argument("--channel", default=None)

    replay = sub.add_parser("replay", help="管理员触发：把条目推给 Lux")
    replay.add_argument("--id", dest="ids", type=int, nargs="*", default=None, help="指定 item id")
    replay.add_argument("--all", action="store_true", help="所有 held/failed/pending 条目")
    replay.add_argument("--status", default="held,failed,pending", help="按状态选（逗号分隔）")
    replay.add_argument("--limit", type=int, default=200)
    replay.add_argument("--dry-run", action="store_true", help="只列出，不投递")
    return parser


def _load(args) -> config_mod.SidecarConfig:
    config = config_mod.load(args.settings)
    if args.log_level:
        config = dataclasses.replace(config, log_level=args.log_level.upper())
    logsafe.configure(config.log_level, config.secrets())
    return config


def _cmd_status(config: config_mod.SidecarConfig) -> int:
    print(json.dumps(config.masked(), ensure_ascii=False, indent=2))
    session = config.session_path
    if session.exists():
        mode = oct(session.stat().st_mode & 0o777)
        print(f"session 存在: True（权限 {mode}）")
    else:
        print("session 存在: False")
    if config.sink == config_mod.SINK_INBOX:
        inbox = Path(config.inbox_dir)
        if inbox.is_dir():
            print(f"inbox: {inbox}（{len(list(inbox.glob('*.txt')))} 个待消费文件）")
        else:
            print(f"inbox: {inbox}（目录不存在）")
    for warning in config_mod.runtime_warnings(config):
        print(f"⚠️  {warning}")
    store = store_mod.Store(config.state_db)
    try:
        stats = store.stats()
        print("状态库: " + json.dumps(stats, ensure_ascii=False))
        if stats.get("dead"):
            print(
                f"死信: {stats['dead']} 条（原因 {json.dumps(stats['dead_reasons'], ensure_ascii=False)}；"
                "`main.py replay --id <id>` 可人工再试）"
            )
    finally:
        store.close()
    try:
        print("下游: " + json.dumps(sink_mod.build(config).describe(), ensure_ascii=False))
    except OSError as error:
        print(f"下游: 不可用（{error}）")
    return 0


def _cmd_check(config: config_mod.SidecarConfig) -> int:
    for warning in config_mod.runtime_warnings(config):
        print(f"⚠️  {warning}")
    problems = config_mod.runtime_problems(config)
    if problems:
        for problem in problems:
            print(f"❌ {problem}")
        return 1
    print("✅ 配置可用")
    return 0


def _cmd_held(config: config_mod.SidecarConfig, limit: int, channel: str | None) -> int:
    store = store_mod.Store(config.state_db)
    try:
        items = store.items_by_status((store_mod.STATUS_HELD,), limit=limit, channel=channel)
        if not items:
            print("没有被挂起的条目")
            return 0
        for item in items:
            print(
                f"#{item['id']:<6} {item['created_at']} {item['channel']} {item['msg_id']} "
                f"[{item['kind']}/{item['mode']}/{item['reason']}] size={item['size_bytes']} "
                f"{item['url'][:120]}"
            )
        return 0
    finally:
        store.close()


async def _cmd_replay(config: config_mod.SidecarConfig, args) -> int:
    statuses = tuple(part.strip() for part in str(args.status).split(",") if part.strip())
    known = (
        store_mod.STATUS_HELD,
        store_mod.STATUS_FAILED,
        store_mod.STATUS_PENDING,
        store_mod.STATUS_SKIPPED,
        store_mod.STATUS_DELIVERED,
        store_mod.STATUS_DEAD,
    )
    unknown = [status for status in statuses if status not in known]
    if unknown:
        print(f"未知状态: {unknown}", file=sys.stderr)
        return 2
    if not args.ids and not args.all:
        print("要么给 --id，要么加 --all", file=sys.stderr)
        return 2
    if args.dry_run:
        store = store_mod.Store(config.state_db)
        try:
            ids = args.ids or [int(item["id"]) for item in store.items_by_status(statuses, limit=args.limit)]
            print(f"dry-run：将投递 {len(ids)} 条 {ids[:50]}")
            return 0
        finally:
            store.close()
    count = await runner.deliver_pending(config, args.ids, statuses, args.limit)
    print(f"已处理 {count} 条")
    return 0


async def _run_with_signals(coroutine) -> None:
    """容器收到 SIGTERM 时优雅断开，别把 session 写坏。"""
    loop = asyncio.get_running_loop()
    task = asyncio.current_task()
    for sig in (signal.SIGTERM, signal.SIGINT):
        try:
            loop.add_signal_handler(sig, task.cancel)
        except (NotImplementedError, AttributeError):  # Windows
            continue
    try:
        await coroutine
    except asyncio.CancelledError:
        log.info("收到退出信号，正在断开…")


def main(argv: list[str] | None = None) -> int:
    # session/状态库/临时文件一律不给同组和其他用户读
    os.umask(0o077)
    args = _build_parser().parse_args(argv)
    try:
        config = _load(args)
    except config_mod.ConfigError as error:
        print(f"❌ {error}", file=sys.stderr)
        return 2
    except filters.ModeError as error:
        print(f"❌ {error}", file=sys.stderr)
        return 2

    try:
        if args.command == "status":
            return _cmd_status(config)
        if args.command == "check":
            return _cmd_check(config)
        if args.command == "held":
            return _cmd_held(config, args.limit, args.channel)
        if args.command == "run":
            asyncio.run(_run_with_signals(runner.run(config)))
            return 0
        if args.command == "replay":
            return asyncio.run(_cmd_replay(config, args))
        print(f"未知命令: {args.command}", file=sys.stderr)
        return 2
    except runner.RunnerError as error:
        log.error("%s", error)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
