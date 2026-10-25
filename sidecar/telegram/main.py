#!/usr/bin/env python3
"""Lux Telegram 旁挂入口。

    python main.py run               # 首次运行即登录（验证码提示在日志里），之后常驻监听
    python main.py status            # 配置（打码）+ 状态库统计
    python main.py check             # 只校验配置是否就绪，不联网
    python main.py check --deep      # 配置 + 连一次 Telegram 确认登录态（HEALTHCHECK 用）
    python main.py held              # 看被挂起的条目（手动模式 / 洗版判定未知）
    python main.py replay --all      # 管理员触发：把挂起/失败/pending 的条目推给 Lux

退出码：0 正常，1 check 有硬错误（含 `check --deep` 未登录/连不上），2 配置或登录前置条件缺失。
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
from tgwatch import filters, health, logsafe, runner, sink as sink_mod, store as store_mod

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

    run = sub.add_parser("run", help="常驻监听（首次运行会走登录流程）")
    # Lux 的「测试连接」和容器 healthcheck 都打这个端点；0 表示不监听。
    run.add_argument(
        "--health-port",
        type=int,
        default=int(os.environ.get("LUX_TG_HEALTH_PORT", "0") or 0),
        help="监听 HTTP /healthz 的端口（默认 $LUX_TG_HEALTH_PORT，未设为 0 = 关闭）",
    )
    sub.add_parser("status", help="打印配置（打码）与状态库统计")
    check = sub.add_parser("check", help="校验配置是否就绪；--deep 额外验 Telegram 登录态")
    check.add_argument(
        "--deep",
        action="store_true",
        help="连一次 Telegram，用 is_user_authorized() 确认 session 仍有登录态（healthcheck 用）",
    )

    held = sub.add_parser("held", help="列出被挂起的条目")
    held.add_argument("--limit", type=int, default=50)
    held.add_argument("--channel", default=None)

    replay = sub.add_parser("replay", help="管理员触发：把条目推给 Lux")
    replay.add_argument("--id", dest="ids", type=int, nargs="*", default=None, help="指定 item id")
    replay.add_argument("--all", action="store_true", help="所有 held/failed/pending 条目")
    replay.add_argument("--status", default="held,failed,pending", help="按状态选（逗号分隔）")
    replay.add_argument("--limit", type=int, default=200)
    replay.add_argument("--dry-run", action="store_true", help="只列出，不投递")
    replay.add_argument(
        "--force",
        action="store_true",
        help="忽略回执再发一次（用于确认过 uncertain 那条其实没转存）",
    )

    complete = sub.add_parser(
        "complete", help="补集：按 Lux 的缺集清单在频道里搜对应集（默认 dry-run）"
    )
    complete.add_argument(
        "--submit",
        action="store_true",
        help="真的提交转存（不加只搜索和判定，不写下游）",
    )
    complete.add_argument(
        "--search-limit",
        type=int,
        default=50,
        help="每个剧名在每个频道最多取几条消息（再按集号筛）",
    )
    complete.add_argument(
        "--full",
        action="store_true",
        help="不节流：忽略搜索间隔与单轮预算，一次跑完（连发易触发 Telegram 风控，慎用）",
    )
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


def _cmd_check(config: config_mod.SidecarConfig, deep: bool = False) -> int:
    """`check`：配置就绪；`check --deep`：再加一次真的 Telegram 登录态自检。

    两者都返回 0/1：healthcheck 只看退出码，原因打成一行中文。
    """
    for warning in config_mod.runtime_warnings(config):
        print(f"⚠️  {warning}")
    problems = config_mod.runtime_problems(config)
    if problems:
        for problem in problems:
            print(f"❌ {problem}")
        return 1
    if not deep:
        print("✅ 配置可用")
        return 0
    ok, detail = asyncio.run(runner.check_session(config))
    if not ok:
        print(f"❌ {detail}")
        return 1
    print(f"✅ 配置可用；{detail}")
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
    count = await runner.deliver_pending(config, args.ids, statuses, args.limit, force=args.force)
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
            return _cmd_check(config, deep=args.deep)
        if args.command == "held":
            return _cmd_held(config, args.limit, args.channel)
        if args.command == "run":
            # Started before the worker so Lux can probe during the first login.
            server = health.serve(config, getattr(args, "health_port", 0))
            if server is not None:
                log.info("health endpoint on %s", server.server_address)
            try:
                asyncio.run(_run_with_signals(runner.run(config)))
            finally:
                if server is not None:
                    server.shutdown()
            return 0
        if args.command == "replay":
            return asyncio.run(_cmd_replay(config, args))
        if args.command == "complete":
            if args.full:
                config = dataclasses.replace(
                    config,
                    complete_pause=0.0,
                    complete_max_searches=0,
                    complete_max_hits=0,
                )
            return asyncio.run(
                runner.complete(
                    dataclasses.replace(config, dry_run=not args.submit),
                    submit=args.submit,
                    search_limit=args.search_limit,
                )
            )
        print(f"未知命令: {args.command}", file=sys.stderr)
        return 2
    except runner.RunnerError as error:
        log.error("%s", error)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
