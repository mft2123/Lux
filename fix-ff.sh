#!/bin/bash
W=/tmp/lux-u11
R=/vol5/1000/build/lux
TIP=$(git -C "$W" rev-parse HEAD)
echo "worktree tip: $TIP"
git -C "$R" branch -f fork/phase-a "$TIP"
git -C "$R" branch -f integration/round2 "$TIP"
echo "--- fork/phase-a ---"
git -C "$R" log --oneline -4 fork/phase-a
echo "--- integration/round2 ---"
git -C "$R" rev-parse --short integration/round2
echo "--- verify our commits are on trunk ---"
git -C "$R" merge-base --is-ancestor 495f0edd fork/phase-a && echo "495f0edd on trunk: yes"
git -C "$R" merge-base --is-ancestor f2e4dfba fork/phase-a && echo "f2e4dfba on trunk: yes"
git -C "$R" merge-base --is-ancestor 43f152b1 fork/phase-a && echo "43f152b1 on trunk: yes (expected, shared history)"