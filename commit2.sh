#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(transfer115): recognise 115's real listing and its real auth failures

Found while running the first real-cookie end-to-end:

- The sidecar only looked for an is_dir key, which 115's /files listing never
  sends. Folders carry cid and no fid, files carry fid (their cid is the parent,
  often \"0\"), so every folder looked like a file and the sidecar tried to mkdir
  a 测试库 that already existed, then reported failed.
- check_login ignored the bare False that p115client returns for a dead session,
  and 115 reports an expired session in the body of an HTTP 200
  (state:false, errno 990001). Both went unnoticed, so a job with a dead cookie
  was recorded failed and moved out of the queue instead of staying there as
  auth_required for the next round.

Tests cover the native payload shape and both auth paths (27 sidecar cases).
docs/handoff records the acceptance run and fixes HANDOFF's \"NAS has no
internet\" claim: 115 and the domestic pip mirrors are reachable from the NAS;
only pypi.org and registry-1.docker.io are not." 
git log --oneline -3
echo "--- status ---"
git status --short