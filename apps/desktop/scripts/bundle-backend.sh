#!/usr/bin/env bash
# Bundle the hermes CLI backend with PyInstaller for MSI distribution.
#
# OPTIONAL offline variant (the default distribution is the uv-bootstrap
# scheme: MSI ships the frontend only and gateway.rs runs `uv tool install
# hermes-agent` on first launch). To produce an offline self-contained MSI:
#   1. run this script to build bundled/dist/hermes-backend/
#   2. re-add `"resources": { "bundled/dist/hermes-backend/": "hermes-backend/" }`
#      to tauri.conf.json `bundle`, then `npx tauri build --bundles msi`.
#
# Usage: bash apps/desktop/scripts/bundle-backend.sh
# Requires: Git Bash on Windows (uses $VENV/Scripts/python.exe), uv, network.
set -euo pipefail

# Resolve the repo root relative to this script so the bundle works from any
# checkout path (no hardcoded /e/git/hermes-agent). Script lives at
# <root>/apps/desktop/scripts/, so the root is three levels up.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
cd "$REPO_ROOT"

OUT="apps/desktop/src-tauri/bundled"
VENV="${TEMP:-/tmp}/hermes-bundle-venv"
WORK="${TEMP:-/tmp}/hermes-pyinst-work"

echo "==> creating bundle venv (python 3.13) at $VENV"
rm -rf "$VENV"
uv venv --python 3.13 "$VENV" 2>&1 | tail -2

echo "==> syncing project deps from uv.lock (frozen — honors override-dependencies/exclude-newer)"
# Main deps + inference-provider extras. Lazy-deps (`tools/lazy_deps.py`)
# pip-installs provider SDKs at runtime; inside the sealed PyInstaller
# bundle that cannot work, so ship the common inference providers (anthropic,
# exa, firecrawl, fal, vercel, hindsight) directly. Messaging/voice/wake
# extras are deliberately excluded (large, not needed for chat).
UV_PROJECT_ENVIRONMENT="$VENV" uv sync --frozen --python 3.13 \
  --extra anthropic --extra exa --extra firecrawl --extra fal --extra vercel --extra hindsight 2>&1 | tail -4

echo "==> installing pyinstaller (pinned) into the venv"
uv pip install --python "$VENV" "pyinstaller==6.14.2" 2>&1 | tail -2

# Seed files so the bundled backend resolves model catalogs offline (cold
# cache + no network would otherwise report an empty provider/model list).
# `--add-data SRC;DIR` places SRC under _internal/DIR **with its original
# basename** — so the seed file names must match what the Python side reads
# (`hermes_cli/model-catalog.json`, `agent/models_dev_cache.json`), and the
# target must be the package directory, not a pseudo filename (a filename
# target creates a *directory* named after it, which is_file() rejects).
# Paths are converted to Windows form (cygpath) because PyInstaller is a
# native Windows process and MSYS-style /e/... paths resolve against the
# wrong drive.
MC_SEED_WIN="$(cygpath -w "$REPO_ROOT/website/static/api/model-catalog.json")"
ADD_DATA_ARGS=(--add-data "$MC_SEED_WIN;hermes_cli")
# 2. models.dev seed: reuse the builder's disk cache when present (full
#    provider/model capability DB, ~3.5 MB); skipped on builders without one.
MODELS_DEV_CACHE=""
for cand in "${HERMES_HOME:-}" "$LOCALAPPDATA/Hermes" "$HOME/.hermes"; do
  if [ -n "$cand" ] && [ -f "$cand/models_dev_cache.json" ]; then
    MODELS_DEV_CACHE="$cand/models_dev_cache.json"
    break
  fi
done
if [ -n "$MODELS_DEV_CACHE" ]; then
  MD_SEED_WIN="$(cygpath -w "$MODELS_DEV_CACHE")"
  ADD_DATA_ARGS+=(--add-data "$MD_SEED_WIN;agent")
  echo "==> models.dev seed: $MODELS_DEV_CACHE"
else
  echo "==> no models.dev cache found on this builder; shipping model_catalog seed only"
fi

echo "==> running PyInstaller (this can take 10-30 min)"
rm -rf "$OUT/dist" "$WORK"
mkdir -p "$WORK/entry"
cat > "$WORK/entry/entry.py" << 'EOF'
from hermes_cli.main import main

if __name__ == "__main__":
    main()
EOF
"$VENV/Scripts/python.exe" -m PyInstaller --noconfirm --clean \
  --name hermes-backend \
  --distpath "$OUT/dist" \
  --workpath "$WORK" \
  --specpath "$WORK" \
  --paths "$REPO_ROOT" \
  --copy-metadata anthropic \
  --copy-metadata exa-py \
  --copy-metadata firecrawl-py \
  --copy-metadata fal-client \
  --copy-metadata vercel \
  --copy-metadata hindsight-client \
  "${ADD_DATA_ARGS[@]}" \
  "$WORK/entry/entry.py" \
  --collect-submodules hermes_cli \
  --collect-submodules agent \
  --collect-submodules gateway \
  --collect-submodules tools \
  --collect-submodules cron \
  --collect-submodules hermes_state \
  --collect-submodules hermes_constants \
  --collect-submodules hermes_logging \
  --collect-submodules batch_runner \
  --collect-submodules model_tools \
  --collect-submodules run_agent 2>&1 | tail -6

echo "==> result"
ls -la "$OUT/dist/hermes-backend/" | head -8
du -sh "$OUT/dist/hermes-backend" 2>/dev/null || true
"$OUT/dist/hermes-backend/hermes-backend.exe" --version 2>&1 | head -2
echo "BUNDLE_OK"
