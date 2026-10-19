"""Offline tests for the transfer service (no network, no real 115 account)."""

from __future__ import annotations

import json
import os
import unittest
import tempfile

from p115client.exception import P115AuthenticationError, P115Error

from transfer115.service import (
    STATUS_AUTH_REQUIRED,
    STATUS_FAILED,
    STATUS_TRANSFERRED,
    STATUS_UNSUPPORTED,
    TransferJob,
    TransferService,
    load_cookie,
    parse_share_link,
)


class FakeClient:
    """Stands in for ``P115Client`` and records every call."""

    def __init__(
        self,
        *,
        snap_items=None,
        children=None,
        login_error=None,
        snap_error=None,
        receive_error=None,
    ):
        self.snap_items = list(snap_items or [])
        self.children = dict(children or {})
        self.login_error = login_error
        self.snap_error = snap_error
        self.receive_error = receive_error
        self.calls: list[tuple] = []
        self._next_id = 100

    def login_status(self):
        self.calls.append(("login_status",))
        if self.login_error:
            raise self.login_error
        return {"state": "success", "data": {"login": True}}

    def share_snap(self, payload):
        self.calls.append(("share_snap", dict(payload)))
        if self.snap_error:
            raise self.snap_error
        return {"state": "success", "data": {"list": self.snap_items}}

    def fs_files(self, payload):
        self.calls.append(("fs_files", dict(payload)))
        cid = str(payload.get("cid"))
        return {"state": "success", "data": {"list": self.children.get(cid, [])}}

    def fs_mkdir(self, name, pid=None):
        self.calls.append(("fs_mkdir", name, str(pid)))
        self._next_id += 1
        return {"state": "success", "data": {"file_id": str(self._next_id), "file_name": name}}

    def share_receive(self, payload):
        self.calls.append(("share_receive", dict(payload)))
        if self.receive_error:
            raise self.receive_error
        return {"state": "success", "data": {}}


def job(url="https://115.com/s/sw123abc?password=abcd", target="转存目录", prefix="测试库"):
    return TransferJob(
        job_id="job-1",
        url=url,
        title="某剧",
        archive_root="/testlib",
        target_dir=target,
        cd2_prefix=prefix,
    )


class ParseShareLinkTests(unittest.TestCase):
    def test_password_parameter(self):
        link = parse_share_link("https://115.com/s/sw123abc?password=abcd")
        self.assertEqual(link.share_code, "sw123abc")
        self.assertEqual(link.receive_code, "abcd")

    def test_no_password(self):
        link = parse_share_link("https://115.com/s/sw123abc")
        self.assertEqual(link.share_code, "sw123abc")
        self.assertEqual(link.receive_code, "")

    def test_pwd_alias_and_trailing_slash(self):
        link = parse_share_link("https://115cdn.com/s/sw999/?pwd=1234")
        self.assertEqual(link.share_code, "sw999")
        self.assertEqual(link.receive_code, "1234")

    def test_rejects_other_hosts_and_paths(self):
        self.assertIsNone(parse_share_link("https://example.com/s/sw123"))
        self.assertIsNone(parse_share_link("https://115.com/file/sw123"))
        self.assertIsNone(parse_share_link("magnet:?xt=urn:btih:abc"))
        self.assertIsNone(parse_share_link(""))


class LoadCookieTests(unittest.TestCase):
    def test_reads_cookie_from_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            with open(os.path.join(directory, "archive-settings.json"), "w", encoding="utf-8") as handle:
                json.dump({"transfer115": {"cookie": "UID=1; CID=2; SEID=3; KID=4"}}, handle)
            self.assertEqual(load_cookie(directory), "UID=1; CID=2; SEID=3; KID=4")

    def test_echoed_mask_is_not_a_cookie(self):
        with tempfile.TemporaryDirectory() as directory:
            with open(os.path.join(directory, "archive-settings.json"), "w", encoding="utf-8") as handle:
                json.dump({"transfer115": {"cookie": "********"}}, handle)
            self.assertEqual(load_cookie(directory), "")

    def test_missing_section(self):
        with tempfile.TemporaryDirectory() as directory:
            with open(os.path.join(directory, "archive-settings.json"), "w", encoding="utf-8") as handle:
                json.dump({"telegram": {}}, handle)
            self.assertEqual(load_cookie(directory), "")


class TransferTests(unittest.TestCase):
    def build(self, client, **kwargs):
        return TransferService(cookie="cookie", client_factory=lambda: client, confirm_seconds=0, **kwargs)

    def test_transfers_into_created_target(self):
        client = FakeClient(snap_items=[{"file_id": 11, "file_name": "某剧 第二季", "is_dir": True}])
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        self.assertEqual(result.saved, ["某剧 第二季"])
        mkdirs = [call for call in client.calls if call[0] == "fs_mkdir"]
        self.assertEqual([call[1] for call in mkdirs], ["测试库", "转存目录"])
        receive = [call for call in client.calls if call[0] == "share_receive"][0][1]
        self.assertEqual(receive["share_code"], "sw123abc")
        self.assertEqual(receive["receive_code"], "abcd")
        self.assertEqual(receive["file_id"], "11")
        self.assertEqual(receive["cid"], "102")
        self.assertEqual(receive["is_check"], 0)

    def test_reuses_existing_target_directory(self):
        client = FakeClient(
            snap_items=[{"file_id": 11, "file_name": "某剧", "is_dir": True}],
            children={
                "0": [{"file_id": 7, "file_name": "测试库", "is_dir": True}],
                "7": [{"file_id": 8, "file_name": "转存目录", "is_dir": True}],
            },
        )
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        self.assertFalse([call for call in client.calls if call[0] == "fs_mkdir"])
        receive = [call for call in client.calls if call[0] == "share_receive"][0][1]
        self.assertEqual(receive["cid"], "8")

    def test_multiple_root_items_are_received_together(self):
        client = FakeClient(
            snap_items=[
                {"file_id": 11, "file_name": "第一集", "is_dir": False},
                {"file_id": 12, "file_name": "第二集", "is_dir": False},
            ]
        )
        result = self.build(client).transfer(job(target="转存目录"))

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        receive = [call for call in client.calls if call[0] == "share_receive"][0][1]
        self.assertEqual(receive["file_id"], "11,12")

    def test_dead_cookie_is_auth_required(self):
        client = FakeClient(login_error=P115AuthenticationError("expired"))
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_AUTH_REQUIRED)
        self.assertIn("cookie", result.detail)

    def test_api_error_is_failure(self):
        client = FakeClient(snap_error=P115Error("boom"))
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_FAILED)
        self.assertIn("115 接口错误", result.detail)

    def test_empty_share_is_failure(self):
        client = FakeClient(snap_items=[])
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_FAILED)

    def test_non_share_link_is_unsupported(self):
        client = FakeClient(snap_items=[{"file_id": 11, "file_name": "x", "is_dir": True}])
        result = self.build(client).transfer(job(url="https://115.com/file/abc"))

        self.assertEqual(result.status, STATUS_UNSUPPORTED)
        self.assertFalse(client.calls)

    def test_missing_prefix_still_resolves_target(self):
        client = FakeClient(snap_items=[{"file_id": 11, "file_name": "x", "is_dir": True}])
        result = self.build(client).transfer(job(prefix=""))

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        mkdirs = [call for call in client.calls if call[0] == "fs_mkdir"]
        self.assertEqual([call[1] for call in mkdirs], ["转存目录"])

    def test_confirm_reports_landed_items(self):
        client = FakeClient(snap_items=[{"file_id": 11, "file_name": "某剧", "is_dir": True}])
        service = TransferService(
            cookie="cookie",
            client_factory=lambda: client,
            confirm_seconds=1,
            poll_interval=0.01,
        )
        client.children["102"] = [{"file_id": 5, "file_name": "某剧", "is_dir": True}]
        result = service.transfer(job())

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        self.assertIn("已转存 1 个条目", result.detail)


if __name__ == "__main__":
    unittest.main()
