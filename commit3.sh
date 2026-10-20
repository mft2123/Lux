#!/bin/bash
J=/tmp/c.jar; rm -f $J
curl -s -c $J -X POST http://127.0.0.1:8099/api/v1/auth/login -H "content-type: application/json" \
  -d "{\"username\":\"admin\",\"password\":\"Test@12345a\"}" -o /dev/null
csrf=$(awk '$6=="lux_csrf" {print $7}' $J)
echo "--- unconfigured branch ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":""}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo
echo "--- restore good sidecar ---"
curl -s -b $J -X PUT http://127.0.0.1:8099/api/v1/admin/archive/settings \
  -H "X-CSRF-Token: $csrf" -H "content-type: application/json" \
  -d '{"transfer115":{"sidecar_base":"http://172.17.0.1:8098"}}' -o /dev/null
curl -s -b $J -X POST http://127.0.0.1:8099/api/v1/admin/archive/transfer115/probe -H "X-CSRF-Token: $csrf"; echo

cd /tmp/lux-u11
git add -A
git -c user.name="Lux Hand-off" -c user.email="lux@local" commit -q -m "feat(settings): save each settings group on its own, and test the 115 sidecar from the page

The archive settings page had one save button at the bottom: a half-finished
edit in one group could not be committed without also committing whatever else
was typed, and there was no way to tell whether the 115 sidecar was reachable or
whether the stored cookie still worked.

- Every group (TMDB / paths / wash / notify / listen / 115 transfer) now has its
  own save button and sends only its own keys, so unsaved edits elsewhere stay
  as typed.
- The 115 group gains a 测试连接 button. It cannot run in the browser (the
  sidecar lives on the docker network), so POST .../transfer115/probe asks luxd
  to call {sidecar}/healthz?deep=1 and reports logged_in / auth_required /
  unconfigured / unreachable.
- The form is filled once; later refetches no longer clobber unsaved edits, and
  a successful save merges back only its own group." 
git log --oneline -2