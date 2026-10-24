"""SQLite 状态库：消息去重、条目状态机、失败重试队列。

放在 `/data/state/state.db`（卷），重启后不会重复投递同一条链接。

状态机：
    pending  → 已入库，等待投递（手动模式下先落这里）
    delivered→ 已成功写下游
    held     → 判定需要人工/信息不足（体积未知、库中体积未知）
    failed   → 下游投递失败，等 retry 循环重试
    dead     → 死信：Lux 永久拒绝（400/403）或重试到上限（review SC4），不再自动重试
    skipped  → 判定不投（记原因，不再处理）
"""

from __future__ import annotations

import sqlite3
import threading
from contextlib import closing
from datetime import datetime, timezone
from pathlib import Path

SCHEMA = """
CREATE TABLE IF NOT EXISTS messages (
    msg_key     TEXT PRIMARY KEY,
    channel     TEXT NOT NULL,
    msg_id      INTEGER NOT NULL,
    msg_date    TEXT,
    mode        TEXT NOT NULL,
    link_count  INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL,
    reason      TEXT,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    msg_key      TEXT NOT NULL,
    channel      TEXT NOT NULL,
    msg_id       INTEGER NOT NULL,
    msg_date     TEXT,
    url          TEXT NOT NULL,
    kind         TEXT NOT NULL,
    anchor       TEXT,
    title        TEXT,
    size_bytes   INTEGER,
    mode         TEXT NOT NULL,
    status       TEXT NOT NULL,
    reason       TEXT,
    attempts     INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    delivered_at TEXT,
    UNIQUE (msg_key, url)
);

CREATE INDEX IF NOT EXISTS idx_items_status ON items (status, id);
CREATE INDEX IF NOT EXISTS idx_items_channel ON items (channel, id);

-- 每个频道上次处理到的消息 id：宕机重启后从这里往后补拉，不丢窗口内的消息
CREATE TABLE IF NOT EXISTS channel_state (
    channel          TEXT PRIMARY KEY,
    last_message_id  INTEGER NOT NULL DEFAULT 0,
    updated_at       TEXT NOT NULL
);

-- 回执：一条链接只该被转存一次。planned → submitted 是正常路径；
-- uncertain 表示发出去但不知道结果，永不自动重发、也永不被清理淘汰。
CREATE TABLE IF NOT EXISTS receipts (
    url         TEXT PRIMARY KEY,
    channel     TEXT NOT NULL,
    msg_id      INTEGER NOT NULL,
    title       TEXT,
    status      TEXT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_receipts_status ON receipts (status);
"""

STATUS_PENDING = "pending"
STATUS_DELIVERED = "delivered"
STATUS_HELD = "held"
STATUS_FAILED = "failed"
STATUS_DEAD = "dead"
STATUS_SKIPPED = "skipped"

RETRYABLE = (STATUS_PENDING, STATUS_HELD, STATUS_FAILED)

# ---- 回执三态（daquan 的 telegram receipts）----
# planned   已决定转存，还没发出去
# uncertain 发出去了但结果未知（超时/连接被重置）：**不自动重试**，否则可能转存两次
# submitted Lux 已受理（含 duplicate）：永不再发
RECEIPT_PLANNED = "planned"
RECEIPT_UNCERTAIN = "uncertain"
RECEIPT_SUBMITTED = "submitted"

#: 回执上限。清理只淘汰最老的 submitted，planned/uncertain 一律保留 —— 不确定的
#: 那条正是最需要留在眼前的东西。
RECEIPT_CAP = 10000


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


class Store:
    """单进程、多线程（runner 的 handler 与重试循环）共享，靠内部锁串行化写入。"""

    def __init__(self, path: str | Path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._conn = sqlite3.connect(str(self.path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute("PRAGMA journal_mode=WAL")
            cursor.execute("PRAGMA synchronous=NORMAL")
            cursor.executescript(SCHEMA)
        self._conn.commit()

    # ---- 消息层 ----

    def seen(self, msg_key: str) -> bool:
        with self._lock, closing(self._conn.cursor()) as cursor:
            row = cursor.execute("SELECT 1 FROM messages WHERE msg_key = ?", (msg_key,)).fetchone()
            return row is not None

    def record_message(
        self,
        *,
        msg_key: str,
        channel: str,
        msg_id: int,
        msg_date: str,
        mode: str,
        link_count: int,
        status: str,
        reason: str | None = None,
    ) -> bool:
        """登记一条消息；已登记过返回 False（用于断线重连后的重复事件）。"""
        with self._lock, closing(self._conn.cursor()) as cursor:
            try:
                cursor.execute(
                    "INSERT INTO messages (msg_key, channel, msg_id, msg_date, mode, link_count, status, reason, created_at)"
                    " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (msg_key, channel, msg_id, msg_date, mode, link_count, status, reason, _now()),
                )
            except sqlite3.IntegrityError:
                return False
            self._conn.commit()
            return True

    # ---- 条目层 ----

    def add_item(
        self,
        *,
        msg_key: str,
        channel: str,
        msg_id: int,
        msg_date: str,
        url: str,
        kind: str,
        anchor: str,
        title: str,
        size_bytes: int | None,
        mode: str,
        status: str,
        reason: str | None = None,
    ) -> int | None:
        """同一 (message, url) 只入库一次；重复返回 None。"""
        with self._lock, closing(self._conn.cursor()) as cursor:
            try:
                cursor.execute(
                    "INSERT INTO items (msg_key, channel, msg_id, msg_date, url, kind, anchor, title,"
                    " size_bytes, mode, status, reason, attempts, created_at)"
                    " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)",
                    (
                        msg_key,
                        channel,
                        msg_id,
                        msg_date,
                        url,
                        kind,
                        anchor,
                        title,
                        size_bytes,
                        mode,
                        status,
                        reason,
                        _now(),
                    ),
                )
            except sqlite3.IntegrityError:
                return None
            item_id = int(cursor.lastrowid)
            self._conn.commit()
            return item_id

    def set_status(self, item_id: int, status: str, reason: str | None = None, *, bump_attempts: bool = False) -> None:
        delivered_at = _now() if status == STATUS_DELIVERED else None
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute(
                "UPDATE items SET status = ?, reason = COALESCE(?, reason),"
                " attempts = attempts + ?, delivered_at = COALESCE(?, delivered_at) WHERE id = ?",
                (status, reason, 1 if bump_attempts else 0, delivered_at, item_id),
            )
            self._conn.commit()

    def get_item(self, item_id: int) -> dict | None:
        with self._lock, closing(self._conn.cursor()) as cursor:
            row = cursor.execute("SELECT * FROM items WHERE id = ?", (item_id,)).fetchone()
            return dict(row) if row else None

    def items_by_status(self, statuses: tuple[str, ...], limit: int = 200, channel: str | None = None) -> list[dict]:
        if not statuses:
            return []
        placeholders = ",".join("?" for _ in statuses)
        sql = f"SELECT * FROM items WHERE status IN ({placeholders})"
        params: list = list(statuses)
        if channel:
            sql += " AND channel = ?"
            params.append(channel)
        sql += " ORDER BY id LIMIT ?"
        params.append(int(limit))
        with self._lock, closing(self._conn.cursor()) as cursor:
            rows = cursor.execute(sql, params).fetchall()
            return [dict(row) for row in rows]

    def promote(self, item_ids: list[int], status: str = STATUS_PENDING) -> int:
        """手动放行：held/pending/failed → pending，让投递循环去推。

        `dead` 也允许被显式放行（`replay --id N`）：死信默认不再自动重试，
        但运维认清原因后可以手动再试一次。
        """
        if not item_ids:
            return 0
        placeholders = ",".join("?" for _ in item_ids)
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute(
                f"UPDATE items SET status = ?, reason = 'released' WHERE id IN ({placeholders})"
                f" AND status IN ('held', 'pending', 'failed', 'dead')",
                [status, *item_ids],
            )
            changed = cursor.rowcount
            self._conn.commit()
            return int(changed)

    # ---- 回执三态（一条链接只该被转存一次）----

    def receipt(self, url: str) -> dict | None:
        with self._lock, closing(self._conn.cursor()) as cursor:
            row = cursor.execute("SELECT * FROM receipts WHERE url = ?", (url,)).fetchone()
            return dict(row) if row else None

    def plan_receipt(self, url: str, channel: str, msg_id: int, title: str | None) -> str:
        """把链接登记成 planned，并返回**登记前的状态**（没有则空串）。

        调用方靠返回值决定要不要发：`submitted` 直接跳过，`uncertain` 也不自动
        重发（先人工确认），其余照常。
        """
        now = _now()
        with self._lock, closing(self._conn.cursor()) as cursor:
            row = cursor.execute("SELECT status FROM receipts WHERE url = ?", (url,)).fetchone()
            previous = str(row["status"]) if row else ""
            if previous in (RECEIPT_SUBMITTED, RECEIPT_UNCERTAIN):
                cursor.execute(
                    "UPDATE receipts SET attempts = attempts + 1, updated_at = ? WHERE url = ?",
                    (now, url),
                )
            elif previous == RECEIPT_PLANNED:
                cursor.execute(
                    "UPDATE receipts SET attempts = attempts + 1, updated_at = ? WHERE url = ?",
                    (now, url),
                )
            else:
                cursor.execute(
                    "INSERT INTO receipts (url, channel, msg_id, title, status, attempts, created_at, updated_at)"
                    " VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
                    (url, channel, int(msg_id), title, RECEIPT_PLANNED, now, now),
                )
            self._conn.commit()
            return previous

    def mark_receipt(self, url: str, status: str) -> None:
        if status not in (RECEIPT_PLANNED, RECEIPT_UNCERTAIN, RECEIPT_SUBMITTED):
            raise ValueError(f"未知回执状态: {status}")
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute(
                "UPDATE receipts SET status = ?, updated_at = ? WHERE url = ?",
                (status, _now(), url),
            )
            self._conn.commit()

    def clear_receipt(self, url: str) -> None:
        """人工放行：把回执删掉，下一条重试会重新走 planned。"""
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute("DELETE FROM receipts WHERE url = ?", (url,))
            self._conn.commit()

    def receipt_counts(self) -> dict[str, int]:
        with self._lock, closing(self._conn.cursor()) as cursor:
            rows = cursor.execute(
                "SELECT status, COUNT(*) AS n FROM receipts GROUP BY status"
            ).fetchall()
            return {row["status"]: row["n"] for row in rows}

    def trim_receipts(self, cap: int = RECEIPT_CAP) -> int:
        """超过上限时只淘汰最老的 submitted。

        planned / uncertain 永远保留：未完成的和结果未知的正是要留住的那几条，
        淘汰它们等于允许同一条链接再转存一次。
        """
        with self._lock, closing(self._conn.cursor()) as cursor:
            total = cursor.execute("SELECT COUNT(*) AS n FROM receipts").fetchone()["n"]
            excess = int(total) - int(cap)
            if excess <= 0:
                return 0
            cursor.execute(
                "DELETE FROM receipts WHERE url IN ("
                "  SELECT url FROM receipts WHERE status = ? ORDER BY updated_at ASC, url ASC LIMIT ?"
                ")",
                (RECEIPT_SUBMITTED, excess),
            )
            removed = cursor.rowcount
            self._conn.commit()
            return int(removed)

    def stats(self) -> dict:
        with self._lock, closing(self._conn.cursor()) as cursor:
            messages = cursor.execute("SELECT COUNT(*) AS n FROM messages").fetchone()["n"]
            rows = cursor.execute("SELECT status, COUNT(*) AS n FROM items GROUP BY status").fetchall()
            dead = cursor.execute(
                "SELECT COALESCE(reason, '') AS reason, COUNT(*) AS n FROM items"
                " WHERE status = ? GROUP BY reason ORDER BY n DESC, reason ASC LIMIT 5",
                (STATUS_DEAD,),
            ).fetchall()
            receipts = cursor.execute(
                "SELECT status, COUNT(*) AS n FROM receipts GROUP BY status"
            ).fetchall()
        by_status = {row["status"]: row["n"] for row in rows}
        return {
            "messages": messages,
            "items": sum(by_status.values()),
            "by_status": by_status,
            # 死信要能在 `status` 里看到「多少条 + 为什么」，否则 rollback 时无从下手
            "dead": by_status.get(STATUS_DEAD, 0),
            "dead_reasons": {row["reason"] or "unknown": row["n"] for row in dead},
            "receipts": {row["status"]: row["n"] for row in receipts},
        }

    # ---- 频道游标（宕机期间的消息靠它补拉） ----

    def last_message_id(self, channel: str) -> int:
        with self._lock, closing(self._conn.cursor()) as cursor:
            row = cursor.execute(
                "SELECT last_message_id FROM channel_state WHERE channel = ?", (channel,)
            ).fetchone()
            return int(row["last_message_id"]) if row else 0

    def advance_message_id(self, channel: str, msg_id: int) -> int:
        """只前进不后退；返回写入后的游标。"""
        if msg_id <= 0:
            return self.last_message_id(channel)
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute(
                "INSERT INTO channel_state (channel, last_message_id, updated_at) VALUES (?, ?, ?)"
                " ON CONFLICT(channel) DO UPDATE SET"
                " last_message_id = MAX(last_message_id, excluded.last_message_id),"
                " updated_at = excluded.updated_at",
                (channel, int(msg_id), _now()),
            )
            self._conn.commit()
            row = cursor.execute(
                "SELECT last_message_id FROM channel_state WHERE channel = ?", (channel,)
            ).fetchone()
            return int(row["last_message_id"]) if row else 0

    def close(self) -> None:
        with self._lock:
            self._conn.close()
