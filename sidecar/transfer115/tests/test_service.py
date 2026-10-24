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
    STATUS_SUBMITTED,
    STATUS_TRANSFERRED,
    STATUS_UNSUPPORTED,
    OfflineJob,
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

    def clouddownload_task_add_url(self, payload):
        self.calls.append(("clouddownload_task_add_url", dict(payload)))
        if getattr(self, "offline_error", None):
            raise self.offline_error
        return {"state": "success", "data": {"task_id": "task-9"}}


def offline_job(url="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567", target="离线下载", prefix="测试库"):
    return OfflineJob(
        job_id="job-2",
        url=url,
        title="某片",
        target_dir=target,
        cd2_prefix=prefix,
    )


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

    def test_password_with_poster_note_is_trimmed(self):
        # Real-world paste: the sharer appended a note after the code, and the
        # note arrives percent-encoded inside the password parameter.
        url = (
            "https://115cdn.com/s/sws1yeb3wt5?password=x3c1"
            "%EF%BC%8C%E8%AF%B7%E5%8F%8A%E6%97%B6%E6%A3%80%E6%9F%A5"
            "%E7%BD%91%E7%9B%98%E7%8A%B6%E5%86%B5%E6%88%96%E8%80%85"
            "cookie%E6%98%AF%E5%90%A6%E9%A3%8E%E6%8E%A7%E6%88%96%E8%80%85"
            "%E6%9C%89%E6%95%88%EF%BC%81"
        )
        link = parse_share_link(url)
        self.assertEqual(link.share_code, "sws1yeb3wt5")
        self.assertEqual(link.receive_code, "x3c1")

    def test_password_without_ascii_token_is_empty(self):
        link = parse_share_link("https://115.com/s/sw123abc?password=%E8%AF%B7%E6%A3%80%E6%9F%A5")
        self.assertEqual(link.receive_code, "")

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

    def test_recognises_115_native_directory_shape(self):
        # What 115's own /files listing returns: a folder has ``cid`` and no
        # ``fid``; a file has ``fid`` and its ``cid`` is the parent (often "0").
        # There is no ``is_dir`` key at all.
        client = FakeClient(
            snap_items=[{"fid": "11", "n": "某剧"}],
            children={
                "0": [{"cid": "7", "n": "测试库", "pid": "0"}],
                "7": [{"cid": "8", "n": "转存目录", "pid": "7"}],
            },
        )
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        self.assertFalse([call for call in client.calls if call[0] == "fs_mkdir"])
        receive = [call for call in client.calls if call[0] == "share_receive"][0][1]
        self.assertEqual(receive["cid"], "8")

    def test_file_with_the_target_name_is_not_taken_for_a_directory(self):
        # A stray *file* called 测试库 sits next to the real folder: it must be
        # skipped (its ``cid`` is the parent, so trusting it would walk into the
        # wrong tree), and the existing folder must be used without mkdir.
        client = FakeClient(
            snap_items=[{"fid": "11", "n": "某剧"}],
            children={
                "0": [
                    {"cid": "0", "fid": "99", "n": "测试库"},
                    {"cid": "7", "n": "测试库"},
                ],
                "7": [{"cid": "7b", "n": "转存目录"}],
            },
        )
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_TRANSFERRED)
        self.assertFalse([call for call in client.calls if call[0] == "fs_mkdir"])
        receive = [call for call in client.calls if call[0] == "share_receive"][0][1]
        self.assertEqual(receive["cid"], "7b")

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

    def test_login_status_false_is_auth_required(self):
        # p115client reports a dead session as the bare False, not a dict and not
        # an exception; missing that made a lost job look like a failure.
        class DeadClient(FakeClient):
            def login_status(self):
                self.calls.append(("login_status",))
                return False

        client = DeadClient(snap_items=[{"file_id": 11, "file_name": "某剧"}])
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_AUTH_REQUIRED)
        self.assertFalse([call for call in client.calls if call[0] == "share_snap"])

    def test_session_lost_mid_flight_is_auth_required(self):
        # The same session death also arrives in-body on an HTTP 200.
        client = FakeClient(snap_items=[{"file_id": 11, "file_name": "某剧"}])
        client.fs_files = lambda payload: {
            "state": False,
            "errno": 990001,
            "error": "登录超时，请重新登录。",
        }
        result = self.build(client).transfer(job())

        self.assertEqual(result.status, STATUS_AUTH_REQUIRED)

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


class OfflineDownloadTests(unittest.TestCase):
    """magnet / ed2k go to 115 cloud download, not to share receive."""

    def service(self, client):
        return TransferService(cookie="UID=1; CID=2; SEID=3; KID=4", client_factory=lambda: client)

    def test_magnet_is_submitted_to_cloud_download(self):
        client = FakeClient()
        result = self.service(client).offline(offline_job())

        self.assertEqual(result.status, STATUS_SUBMITTED)
        self.assertIn("离线下载", result.detail)
        calls = [call for call in client.calls if call[0] == "clouddownload_task_add_url"]
        self.assertEqual(len(calls), 1)
        payload = calls[0][1]
        self.assertTrue(payload["url"].startswith("magnet:?"))
        # 目标目录按 cd2_prefix/target_dir 逐级解析，不能传空 cid。
        self.assertTrue(payload["wp_path_id"])
        self.assertNotIn("share_receive", [call[0] for call in client.calls])

    def test_ed2k_is_submitted(self):
        link = "ed2k://|file|%s|1024|0123456789abcdef0123456789abcdef|/" % "某片.mkv"
        client = FakeClient()
        result = self.service(client).offline(offline_job(url=link))

        self.assertEqual(result.status, STATUS_SUBMITTED)
        self.assertTrue([call for call in client.calls if call[0] == "clouddownload_task_add_url"])

    def test_malformed_ed2k_fails_before_touching_115(self):
        client = FakeClient()
        result = self.service(client).offline(offline_job(url="ed2k://|file|某个文件|/"))

        self.assertEqual(result.status, STATUS_FAILED)
        self.assertIn("ed2k 链接格式无效", result.detail)
        self.assertEqual(client.calls, [])

    def test_other_links_are_unsupported(self):
        client = FakeClient()
        result = self.service(client).offline(offline_job(url="https://115.com/s/sw123abc"))

        self.assertEqual(result.status, STATUS_UNSUPPORTED)
        self.assertEqual(client.calls, [])

    def test_dead_cookie_is_auth_required(self):
        client = FakeClient(login_error=P115AuthenticationError("登录状态已失效"))
        result = self.service(client).offline(offline_job())

        self.assertEqual(result.status, STATUS_AUTH_REQUIRED)

    def test_upstream_rejection_is_reported_as_failed(self):
        link = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"

        class RejectingClient(FakeClient):
            def clouddownload_task_add_url(self, payload):
                self.calls.append(("clouddownload_task_add_url", dict(payload)))
                return {"state": False, "error": "该链接已存在"}

        result = self.service(RejectingClient()).offline(offline_job(url=link))

        self.assertEqual(result.status, STATUS_FAILED)
        self.assertIn("该链接已存在", result.detail)

if __name__ == "__main__":
    unittest.main()
