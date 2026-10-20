#!/bin/bash
set -e
cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "fix(transfer115): accept the share front ends 115 actually hands out

115 distributes share links on 115cdn.com / anxia.com / 115.link / 115.com.cn as
often as on 115.com. The sidecar already accepted that family (SHARE_HOSTS) but
the Rust predicate only matched 115.com and *.115.com, so a real link was parked
as unsupported and never reached the sidecar. Both halves now share one list.

The sidecar also learns to trim the poster's note that gets pasted into the
password parameter (?password=x3c1，请及时检查…), and pins python-asynctools to
0.1.4: p115client only asks for >=0.1.4 and the 0.2.2 that resolves today no
longer exports to_list, so the image died at import.

Dockerfile takes PIP_INDEX_URL so the image builds where pypi.org is blocked." 
git log --oneline -2
git status --short | head