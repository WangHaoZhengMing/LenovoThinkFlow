#!/usr/bin/env bash
# GitFlow Release Automation Script for LenovoThinkFlow
# Usage: ./scripts/gitflow-release.sh <version> (e.g. 1.0.2)
set -euo pipefail

if [ $# -lt 1 ]; then
    echo "Usage: $0 <version>  (example: $0 1.0.2)"
    exit 1
fi

RAW_VER="$1"
VER="${RAW_VER#v}"
TAG="v${VER}"

if git rev-parse "${TAG}" >/dev/null 2>&1; then
    echo "Error: Tag ${TAG} already exists!"
    exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
    echo "Error: Working tree is dirty. Please commit or stash changes first."
    exit 1
fi

echo "=== [1/5] Switching to develop & creating release/${VER} ==="
git checkout develop
git pull --ff-only origin develop || true
git checkout -b "release/${VER}"

echo "=== [2/5] Bumping version to ${VER} across workspace ==="
python3 - "${VER}" << 'PY'
import re, sys
from pathlib import Path

ver = sys.argv[1]
for p in [Path("cli/Cargo.toml"), Path("rust-core/Cargo.toml")]:
    txt = p.read_text()
    txt = re.sub(r'^version\s*=\s*"[^"]+"', f'version = "{ver}"', txt, count=1, flags=re.M)
    p.write_text(txt)

b = Path("build.py")
btxt = b.read_text()
btxt = re.sub(r'os\.environ\.get\("THINKFLOW_VERSION",\s*"[^"]+"\)', f'os.environ.get("THINKFLOW_VERSION", "{ver}")', btxt)
b.write_text(btxt)

cpp = Path("kext/ThinkFlow.cpp")
ctxt = cpp.read_text()
ctxt = re.sub(r'#define THINKFLOW_VERSION "[^"]+"', f'#define THINKFLOW_VERSION "{ver}"', ctxt)
cpp.write_text(ctxt)
PY

python3 build.py --version "${VER}" --no-install-local
git add -A
git commit -m "chore(release): bump version to ${TAG}" || true

echo "=== [3/5] Merging release/${VER} -> main & tagging ${TAG} ==="
git checkout main
git pull --ff-only origin main || true
git merge --no-ff "release/${VER}" -m "chore(release): merge release/${VER} into main"
git tag -a "${TAG}" -m "Release ${TAG}"

echo "=== [4/5] Back-merging main -> develop & cleaning up release/${VER} ==="
git checkout develop
git merge --no-ff main -m "chore(release): back-merge ${TAG} into develop"
git branch -d "release/${VER}"

echo "=== [5/5] Pushing main, develop & ${TAG} to origin (triggers GitHub Release) ==="
git push origin main develop "${TAG}"
echo "Done! GitHub Actions is now building and publishing Release ${TAG}."
