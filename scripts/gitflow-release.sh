#!/usr/bin/env bash
# GitFlow Release Automation Script for LenovoThinkFlow
# Usage:
#   ./scripts/gitflow-release.sh <version>               # Standard GitFlow + push tag (triggers GitHub Actions, with local gh fallback)
#   ./scripts/gitflow-release.sh --publish-tag <vX.Y.Z>  # Locally build & publish GitHub Release for an existing tag via `gh`
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT_DIR}"

publish_local_release() {
    local TAG="$1"
    local VER="${TAG#v}"
    local DIST="${ROOT_DIR}/dist"
    local STAGE="${DIST}/LenovoThinkFlow-${TAG}-macOS"

    echo "=== Building Release Artifacts Locally for ${TAG} (Universal CLI + Kext + SSDT) ==="
    rm -rf "${DIST}"
    mkdir -p "${STAGE}"

    python3 build.py --version "${VER}" --no-install-local

    if rustup target list --installed 2>/dev/null | grep -q "aarch64-apple-darwin" && \
       rustup target list --installed 2>/dev/null | grep -q "x86_64-apple-darwin"; then
        cargo build --release --target x86_64-apple-darwin -p thinkflow-cli
        cargo build --release --target aarch64-apple-darwin -p thinkflow-cli
        xcrun lipo -create \
            target/x86_64-apple-darwin/release/thinkflow \
            target/aarch64-apple-darwin/release/thinkflow \
            -output build/thinkflow
        xcrun strip -S build/thinkflow
        CLI_ASSET="${DIST}/thinkflow-macos-universal"
    else
        CLI_ASSET="${DIST}/thinkflow-macos-$(uname -m)"
    fi

    cp -R build/LenovoThinkFlow.kext "${STAGE}/"
    cp -R build/ThinkFlow.kext "${STAGE}/"
    cp build/SSDT-THINKFLOW.aml "${STAGE}/"
    cp build/SSDT-THINKFLOW.dsl "${STAGE}/"
    cp build/thinkflow "${STAGE}/"
    cp LICENSE README.md "${STAGE}/"

    (cd "${DIST}" && ditto -c -k --sequesterRsrc --keepParent "LenovoThinkFlow-${TAG}-macOS" "LenovoThinkFlow-${TAG}-macOS.zip")
    (cd build && ditto -c -k --sequesterRsrc --keepParent LenovoThinkFlow.kext "${DIST}/LenovoThinkFlow-${TAG}-Kext.zip")
    cp build/SSDT-THINKFLOW.aml "${DIST}/SSDT-THINKFLOW.aml"
    cp build/thinkflow "${CLI_ASSET}"
    chmod +x "${CLI_ASSET}"

    (cd "${DIST}" && shasum -a 256 *.zip SSDT-THINKFLOW.aml "$(basename "${CLI_ASSET}")" > SHA256SUMS.txt)

    PREV_TAG=$(git tag --sort=-v:refname | grep -E '^v[0-9]+' | grep -v "^${TAG}$" | head -n 1 || true)
    {
        echo "## LenovoThinkFlow ${TAG}"
        echo ""
        echo "### Included Artifacts"
        echo "- **\`LenovoThinkFlow-${TAG}-macOS.zip\`**: Complete bundle (\`LenovoThinkFlow.kext\`, legacy \`ThinkFlow.kext\`, \`SSDT-THINKFLOW.aml\`, \`thinkflow\` TUI/CLI)"
        echo "- **\`LenovoThinkFlow-${TAG}-Kext.zip\`**: Standalone IOKit kernel extension"
        echo "- **\`SSDT-THINKFLOW.aml\`**: ACPI helper table (\`TFIN\` / \`TFSL\` S0ix handoff / \`TFPL\`)"
        echo "- **\`$(basename "${CLI_ASSET}")\`**: Standalone interactive Ratatui TUI & CLI binary"
        echo "- **\`SHA256SUMS.txt\`**: SHA-256 checksums"
        echo ""
        echo "### Changelog"
        if [ -n "${PREV_TAG}" ]; then
            git log "${PREV_TAG}..${TAG}" --pretty=format:"- %s (%h)" --no-merges
        else
            git log "${TAG}" --pretty=format:"- %s (%h)" --no-merges
        fi
        echo ""
    } > "${DIST}/RELEASE_NOTES.md"

    if gh release view "${TAG}" >/dev/null 2>&1; then
        gh release upload "${TAG}" --clobber \
            "${DIST}/LenovoThinkFlow-${TAG}-macOS.zip" \
            "${DIST}/LenovoThinkFlow-${TAG}-Kext.zip" \
            "${DIST}/SSDT-THINKFLOW.aml" \
            "${CLI_ASSET}" \
            "${DIST}/SHA256SUMS.txt"
    else
        gh release create "${TAG}" \
            --title "LenovoThinkFlow ${TAG}" \
            --notes-file "${DIST}/RELEASE_NOTES.md" \
            "${DIST}/LenovoThinkFlow-${TAG}-macOS.zip" \
            "${DIST}/LenovoThinkFlow-${TAG}-Kext.zip" \
            "${DIST}/SSDT-THINKFLOW.aml" \
            "${CLI_ASSET}" \
            "${DIST}/SHA256SUMS.txt"
    fi
}

if [ "${1:-}" = "--publish-tag" ]; then
    if [ $# -lt 2 ]; then
        echo "Usage: $0 --publish-tag <vX.Y.Z>"
        exit 1
    fi
    publish_local_release "$2"
    exit 0
fi

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

echo "=== [5/5] Pushing main, develop & ${TAG} to origin ==="
git push origin main
git push origin develop
git push origin "${TAG}"

# Check after 6s if GitHub Actions runner failed to start (e.g. runner billing lock), and auto-fallback to local gh release
sleep 6
LATEST_CONCL=$(gh run list -L 1 --json conclusion -q '.[0].conclusion' 2>/dev/null || true)
if [ "${LATEST_CONCL}" = "failure" ] && ! gh release view "${TAG}" >/dev/null 2>&1; then
    echo "Notice: Cloud Actions runner unavailable; falling back to local macOS build & gh release upload..."
    git checkout main
    publish_local_release "${TAG}"
    git checkout develop
fi

echo "Done! Release ${TAG} is live."
