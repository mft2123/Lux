#!/usr/bin/env python3
"""Audit (and optionally repair) the local-NFO actor credits of a Lux config dir.

Why this exists
---------------
The actor credits of an item are written in one chunk per batch. When that
chunk fails, the transaction rolls back *and* the item's `people.json` relation
snapshot used to stay "current", so no later scan would ever re-persist the
credits: the item kept zero credits forever while the UI showed its cast from
the snapshot. Fixed in the runtime, but data written before the fix can still
be in that state.

What it checks
--------------
For every `people.json` under `<config>/metadata/library/**/<item_id>/`:

  MISSING_CREDITS  the relation lists actors but `person_credits` has none
  COUNT_MISMATCH   the relation lists N actors but `person_credits` has M != N
  OK               the counts agree

It also reports items that have credits but no relation snapshot (harmless: the
next NFO pass rewrites the relation) and stale snapshots whose item no longer
exists.

Repair
------
`--fix` deletes the `people.json` of the affected items. That drops only the
cached relation, so the next NFO scan re-persists the credits; no media or
database rows are touched. Run without `--fix` first: it is read-only.

Usage
-----
  # read-only report
  python3 actor-credits-audit.py --config-dir /config
  # invalidate the affected relations
  python3 actor-credits-audit.py --config-dir /config --fix

After `--fix`, the credits come back the next time the item is enriched. Note
that the current build does NOT re-enrich unchanged items on a normal scan or
reconcile (verified on the test instance: touching or editing the NFO and
re-scanning left the item alone). To actually restore them, force a full
re-enrichment, e.g. rebuild the library (delete it and add the same root back);
that was verified to restore every credit.

Exit code: 0 when every relation is consistent (or everything was repaired),
1 when problems remain, 2 on a usage/IO error.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from pathlib import Path

METADATA_DIR = "metadata"
LIBRARY_DIR = "library"
RELATION_FILE = "people.json"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audit local-NFO actor credits against the people.json snapshots.",
    )
    parser.add_argument(
        "--config-dir",
        default=os.environ.get("LUX_CONFIG_DIR", "/config"),
        help="Lux config dir (default: $LUX_CONFIG_DIR or /config)",
    )
    parser.add_argument(
        "--db",
        default=None,
        help="SQLite database (default: <config-dir>/lux.db)",
    )
    parser.add_argument(
        "--fix",
        action="store_true",
        help="delete the people.json of inconsistent items so the next NFO scan re-persists their credits",
    )
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    return parser.parse_args()


def relation_paths(config_dir: Path):
    root = config_dir / METADATA_DIR / LIBRARY_DIR
    if not root.is_dir():
        return
    for path in root.rglob(RELATION_FILE):
        if path.is_file():
            yield path


def load_relation(path: Path):
    """Return (item_id, actor_count) or None when the snapshot is unreadable."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    item_id = document.get("itemId") or path.parent.name
    actors = document.get("actors")
    if not isinstance(actors, list):
        return None
    return str(item_id), len(actors)


def credit_counts(connection: sqlite3.Connection) -> dict[str, int]:
    rows = connection.execute(
        "SELECT item_id, COUNT(*) FROM person_credits GROUP BY item_id"
    ).fetchall()
    return {str(item_id): int(count) for item_id, count in rows}


def item_exists(connection: sqlite3.Connection, item_id: str) -> bool:
    row = connection.execute(
        "SELECT 1 FROM media_items WHERE id = ? AND removed_at IS NULL", (item_id,)
    ).fetchone()
    return row is not None


def main() -> int:
    args = parse_args()
    config_dir = Path(args.config_dir)
    db_path = Path(args.db) if args.db else config_dir / "lux.db"
    if not db_path.is_file():
        print(f"error: database not found: {db_path}", file=sys.stderr)
        return 2
    if not config_dir.is_dir():
        print(f"error: config dir not found: {config_dir}", file=sys.stderr)
        return 2

    try:
        connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    except sqlite3.Error as error:
        print(f"error: cannot open {db_path}: {error}", file=sys.stderr)
        return 2

    credits = credit_counts(connection)
    findings: list[dict] = []
    stale: list[dict] = []
    orphan: list[str] = []
    seen_items: set[str] = set()

    for path in relation_paths(config_dir):
        loaded = load_relation(path)
        if loaded is None:
            findings.append(
                {"item_id": None, "state": "UNREADABLE", "path": str(path), "actors": 0, "credits": 0}
            )
            continue
        item_id, actors = loaded
        seen_items.add(item_id)
        stored = credits.get(item_id, 0)
        if not item_exists(connection, item_id):
            stale.append({"item_id": item_id, "path": str(path), "actors": actors, "credits": stored})
            continue
        if actors == 0 and stored == 0:
            continue
        if stored == 0 and actors > 0:
            findings.append(
                {"item_id": item_id, "state": "MISSING_CREDITS", "path": str(path), "actors": actors, "credits": 0}
            )
        elif stored != actors:
            findings.append(
                {"item_id": item_id, "state": "COUNT_MISMATCH", "path": str(path), "actors": actors, "credits": stored}
            )

    for item_id in credits:
        if item_id not in seen_items:
            orphan.append(item_id)

    repaired: list[str] = []
    if args.fix:
        for finding in findings:
            if finding["item_id"] is None:
                continue
            path = Path(finding["path"])
            try:
                path.unlink()
                repaired.append(finding["item_id"])
            except OSError as error:
                print(f"error: cannot delete {path}: {error}", file=sys.stderr)

    if args.json:
        print(
            json.dumps(
                {
                    "config_dir": str(config_dir),
                    "database": str(db_path),
                    "checked_relations": len(seen_items),
                    "problems": findings,
                    "stale_relations": stale,
                    "orphan_credits": orphan,
                    "repaired": repaired,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    else:
        print(f"config dir : {config_dir}")
        print(f"database   : {db_path}")
        print(f"relations  : {len(seen_items)} checked, {len(stale)} stale")
        if not findings:
            print("result     : OK — every actor relation matches its person_credits rows")
        else:
            print(f"result     : {len(findings)} inconsistent relation(s)")
            for finding in findings:
                if finding["item_id"] is None:
                    print(f"  UNREADABLE       {finding['path']}")
                    continue
                print(
                    f"  {finding['state']:<16} {finding['item_id']}  "
                    f"actors={finding['actors']} credits={finding['credits']}"
                )
        if orphan:
            print(f"note       : {len(orphan)} item(s) have credits but no relation snapshot (rewritten by the next scan)")
        if args.fix:
            print(f"repaired   : {len(repaired)} relation(s) invalidated — run a library scan to re-persist their credits")
        elif findings:
            print("next step  : re-run with --fix, then force a full re-enrichment (a plain scan does not re-enrich unchanged items; rebuilding the library does)")

    connection.close()
    if findings and not repaired:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
