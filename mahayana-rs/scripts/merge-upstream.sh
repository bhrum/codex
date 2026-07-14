#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if [[ -n "$(git status --short)" ]]; then
  echo "Refusing to merge upstream into a dirty Mahayana checkout." >&2
  exit 2
fi
if ! git remote get-url upstream >/dev/null 2>&1; then
  git remote add upstream https://github.com/openai/codex.git
fi

git fetch upstream main
git merge --no-ff upstream/main

cat <<'EOF'
Upstream merged. Before updating mahayana-rs/UPSTREAM.lock:
  1. Review: git diff ORIG_HEAD...HEAD -- codex-rs
  2. Run focused Mahayana checks and platform contracts.
  3. Record the merged upstream commit and review date.
EOF
