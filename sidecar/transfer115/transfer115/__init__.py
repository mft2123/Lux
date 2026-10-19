"""transfer115 — the 115 share-link transfer sidecar for Lux.

It logs into the operator's 115 account with a web cookie and receives share
links into a fixed target directory, so CloudDrive2 (which mounts that account)
shows the files and Lux's archive pipeline picks them up from 转存目录.
"""

from .service import (
    STATUS_AUTH_REQUIRED,
    STATUS_FAILED,
    STATUS_TRANSFERRED,
    STATUS_UNSUPPORTED,
    TransferJob,
    TransferResult,
    TransferService,
    load_cookie,
    parse_share_link,
)

__all__ = [
    "STATUS_AUTH_REQUIRED",
    "STATUS_FAILED",
    "STATUS_TRANSFERRED",
    "STATUS_UNSUPPORTED",
    "TransferJob",
    "TransferResult",
    "TransferService",
    "load_cookie",
    "parse_share_link",
]
