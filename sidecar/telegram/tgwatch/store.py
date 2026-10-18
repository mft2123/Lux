"""SQLite 状态库：消息去重、条目状态机、失败重试队列。

放在 `/data/state/state.db`（卷），重启后不会重复投递同一条链接。

状态机：
    pending  → 已入库，等待投递（手动模式下先落这里）
    delivered→ 已成功写下游
    held     → 判定需要人工/信息不足（体积未知、库中体积未知）
    failed   → 下游投递失败，等 retry 循环重试
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
"""

STATUS_PENDING = "pending"
STATUS_DELIVERED = "delivered"
STATUS_HELD = "held"
STATUS_FAILED = "failed"
STATUS_SKIPPED = "skipped"

RETRYABLE = (STATUS_PENDING, STATUS_HELD, STATUS_FAILED)


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
        """手动放行：held/pending → pending，让投递循环去推。"""
        if not item_ids:
            return 0
        placeholders = ",".join("?" for _ in item_ids)
        with self._lock, closing(self._conn.cursor()) as cursor:
            cursor.execute(
                f"UPDATE items SET status = ?, reason = 'released' WHERE id IN ({placeholders})"
                f" AND status IN ('held', 'pending', 'failed')",
                [status, *item_ids],
            )
            changed = cursor.rowcount
            self._conn.commit()
            return int(changed)

    def stats(self) -> dict:
        with self._lock, closing(self._conn.cursor()) as cursor:
            messages = cursor.execute("SELECT COUNT(*) AS n FROM messages").fetchone()["n"]
            rows = cursor.execute("SELECT status, COUNT(*) AS n FROM items GROUP BY status").fetchall()
        by_status = {row["status"]: row["n"] for row in rows}
        return {"messages": messages, "items": sum(by_status.values()), "by_status": by_status}

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
