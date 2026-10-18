"""测试辅助：自己造临时目录，不用 tempfile。

原因：Windows 上 `tempfile.mkdtemp` 造出来的目录带 0700 语义的 DACL，
在 DSH 的 workspace-write 沙箱里子进程无法往里面写文件（父目录写得进、
新目录写不进）。`os.makedirs` 的继承权限正常，所以这里自己拼目录名。
"""

from __future__ import annotations

import contextlib
import os
import shutil
import uuid
from pathlib import Path

WORK_ROOT = Path(__file__).resolve().parent / ".work"


def mkdtemp() -> Path:
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    path = WORK_ROOT / f"t{uuid.uuid4().hex[:12]}"
    os.makedirs(path, exist_ok=False)
    return path


@contextlib.contextmanager
def temp_dir():
    path = mkdtemp()
    try:
        yield path
    finally:
        cleanup(path)


def cleanup(path: Path) -> None:
    shutil.rmtree(path, ignore_errors=True)
