#!/bin/bash
set -e
R=/vol5/1000/build/lux
git -C "$R" branch -f fork/phase-a f2e4dfba
git -C "$R" branch -f integration/round2 f2e4dfba
echo "--- trunk ---"
git -C "$R" log --oneline -3 fork/phase-a
echo "--- integration/round2 ---"
git -C "$R" rev-parse --short integration/round2
echo "--- worktrees ---"
git -C "$R" worktree list