#Requires -Version 5.1
<#
.SYNOPSIS
  Build the bundled Hermes backend for the desktop MSI (python-build-standalone).

.DESCRIPTION
  Produces apps/desktop/backend-dist/hermes-backend/ containing:
    python/     — a relocatable python-build-standalone CPython runtime
                  (fetched via `uv python install`, i.e. the same distribution
                  uv itself manages), with hermes' third-party dependencies
                  installed straight into its site-packages (no venv → no
                  absolute paths → the folder stays relocatable under
                  Program Files)
    app/        — the hermes source layout: top-level packages
                  (agent, tools, hermes_cli, gateway, ...), the single-file
                  py-modules, and the PROJECT_ROOT-resolved asset dirs
                  (locales, skills, optional-skills, optional-mcps)
    launch.py   — entry-point shim that puts app/ on sys.path and calls
                  hermes_cli.main:main

  Why a pseudo-source layout instead of a proper wheel install:
  hermes resolves runtime assets (locales, bundled skills, ...) relative to
  the parent of the package tree (PROJECT_ROOT), and setup.py deliberately
  blocks wheel builds for non-Nix builds. Shipping the source layout side
  steps both: PROJECT_ROOT = <bundle>/app and every asset lookup lands
  exactly as in a dev checkout.

  The whole folder ships as a Tauri MSI resource (see tauri.conf.json) and is
  spawned by src-tauri/src/gateway.rs as:

      python.exe launch.py serve --host 127.0.0.1 --port 0

  Because the runtime is a full CPython (not a frozen/PyInstaller binary),
  FastAPI/uvicorn/dynamic imports/lazy-deps all behave exactly as in dev.

  Run this BEFORE `npx tauri build`. `tauri dev` also picks the bundle up from
  the repo layout via the dev fallback path in gateway.rs.

.PARAMETER PythonVersion
  Python version spec for `uv python install` (must satisfy the project's
  `requires-python = ">=3.11,<3.14"`). Default: 3.12.

.PARAMETER Clean
  Remove the output directory before building.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/build-backend.ps1

.NOTES
  If the python-build-standalone download is slow (GitHub releases), point uv
  at a mirror first:
    $env:UV_PYTHON_INSTALL_MIRROR = "https://ghproxy.cn/https://github.com/astral-sh/python-build-standalone/releases/download"
#>
param(
  [string]$PythonVersion = "3.12",
  [switch]$Clean
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $repoRoot 'apps\desktop\backend-dist\hermes-backend'
$pythonDir = Join-Path $outDir 'python'
$appDir = Join-Path $outDir 'app'

# Auto-discover the importable source layout from the checkout instead of
# mirroring pyproject.toml by hand — an upstream merge that adds a top-level
# module must not silently ship a backend that crashes on import (that is
# exactly how `hermes_state_holders` went missing once).
#
#   packages: top-level dirs carrying __init__.py, minus `tests`
#   modules : top-level *.py, minus build/test scaffolding (setup.py)
$pkgDirs = Get-ChildItem $repoRoot -Directory |
  Where-Object { (Test-Path (Join-Path $_.FullName '__init__.py')) -and ($_.Name -ne 'tests') } |
  Select-Object -ExpandProperty Name
$pyModules = Get-ChildItem $repoRoot -Filter *.py -File |
  Where-Object { $_.Name -notin @('setup.py') } |
  Select-Object -ExpandProperty BaseName
# Asset dirs resolved against PROJECT_ROOT (parent of the package tree) at
# runtime — see agent/i18n.py and hermes_constants.py.
$assetDirs = @('locales', 'skills', 'optional-skills', 'optional-mcps')

# --- locate uv --------------------------------------------------------------
$uv = (Get-Command uv -ErrorAction SilentlyContinue).Source
if (-not $uv) {
  foreach ($p in @(
      (Join-Path $env:USERPROFILE '.local\bin\uv.exe'),
      (Join-Path $env:USERPROFILE '.cargo\bin\uv.exe')
    )) {
    if (Test-Path $p) { $uv = $p; break }
  }
}
if (-not $uv) { throw "uv not found on PATH or in ~/.local/bin. Install it first: https://docs.astral.sh/uv/" }
Write-Host "[build-backend] uv: $uv"

# --- 1. fetch the managed python-build-standalone runtime -------------------
Write-Host "[build-backend] ensuring managed Python $PythonVersion ..."
& $uv python install $PythonVersion
if ($LASTEXITCODE -ne 0) { throw "uv python install $PythonVersion failed" }

# `uv python find` prints the interpreter path, e.g.
#   %LOCALAPPDATA%\uv\python\cpython-3.12.x-windows-x86_64-none\python\python.exe
$managedPython = (& $uv python find $PythonVersion | Select-Object -First 1)
if (-not $managedPython -or -not (Test-Path $managedPython)) {
  throw "could not resolve the managed Python interpreter path (got: '$managedPython')"
}
$srcDir = Split-Path -Parent $managedPython
Write-Host "[build-backend] managed runtime: $srcDir"

# --- 2. stage the runtime ----------------------------------------------------
if ($Clean -and (Test-Path $outDir)) { Remove-Item $outDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $pythonDir | Out-Null

Write-Host "[build-backend] copying python runtime -> $pythonDir"
# robocopy exit codes 0-7 are success; >=8 is failure. Resolve by absolute path
# (PATH may be minimal), with a Copy-Item fallback.
$robocopy = Join-Path $env:SystemRoot 'System32\Robocopy.exe'
function Copy-Tree($src, $dst) {
  if (Test-Path $robocopy) {
    & $robocopy $src $dst /E /NFL /NDL /NJH /NJS /NP /XD __pycache__ | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed for $src (exit $LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
  } else {
    Copy-Item -Path (Join-Path $src '*') -Destination $dst -Recurse -Force
  }
}
Copy-Tree $srcDir $pythonDir

$stagingPython = Join-Path $pythonDir 'python.exe'
if (-not (Test-Path $stagingPython)) { throw "staged python.exe missing at $stagingPython" }

# uv marks its managed interpreters EXTERNALLY-MANAGED so `uv pip install`
# refuses to touch them. Our staged copy is ours now — strip the marker (and
# uv's receipt) so deps can be installed straight into site-packages.
foreach ($rel in @('Lib\EXTERNALLY-MANAGED', 'uv-receipt.toml')) {
  $marker = Join-Path $pythonDir $rel
  if (Test-Path $marker) {
    Remove-Item $marker -Force
    Write-Host "[build-backend] removed $rel"
  }
}

# --- 3. stage the source layout ----------------------------------------------
New-Item -ItemType Directory -Force -Path $appDir | Out-Null
Write-Host "[build-backend] copying source packages -> $appDir"
foreach ($dir in ($pkgDirs + $assetDirs)) {
  $src = Join-Path $repoRoot $dir
  if (-not (Test-Path $src)) {
    Write-Host "[build-backend]   (skipping absent $dir)"
    continue
  }
  Copy-Tree $src (Join-Path $appDir $dir)
}
foreach ($mod in $pyModules) {
  $src = Join-Path $repoRoot "$mod.py"
  if (Test-Path $src) { Copy-Item $src $appDir -Force }
  else { Write-Warning "[build-backend] py-module missing: $mod.py" }
}

# --- 4. install third-party deps into the runtime ----------------------------
# `-r pyproject.toml` installs ONLY [project].dependencies (not the project
# itself, whose wheel build is guarded — see setup.py). Exact pins from
# pyproject.toml apply, so the dep set is reproducible.
Write-Host "[build-backend] installing dependencies (a few minutes) ..."
Push-Location $repoRoot
try {
  & $uv pip install --python $stagingPython -r pyproject.toml
  if ($LASTEXITCODE -ne 0) { throw "uv pip install failed (exit $LASTEXITCODE)" }
} finally {
  Pop-Location
}

# --- 5. entry-point shim ------------------------------------------------------
# gateway.rs spawns: python.exe launch.py serve --host 127.0.0.1 --port 0
$launchPy = @'
"""Bundled Hermes backend entry point.

Spawned by src-tauri/src/gateway.rs as:
    python.exe launch.py serve --host 127.0.0.1 --port 0

Puts the bundled pseudo-source layout (app/) first on sys.path so that
hermes' PROJECT_ROOT resolution (parent of the hermes_cli package) points
inside the bundle and every asset lookup (locales/, skills/, ...) resolves
exactly as in a dev checkout. All argv after the script path is forwarded
to the hermes CLI.
"""
import os
import sys

# The MSI resource dir under Program Files is read-only; keep CPython from
# trying (and failing) to write __pycache__ there.
os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(_HERE, "app"))

from hermes_cli.main import main  # noqa: E402

if __name__ == "__main__":
    sys.exit(main())
'@
Set-Content -Path (Join-Path $outDir 'launch.py') -Value $launchPy -Encoding utf8

# --- 6. trim the fat ----------------------------------------------------------
# Drop the stdlib test suite (~40 MB) and generated bytecode caches. Neither is
# ever imported at runtime.
$libTest = Join-Path $pythonDir 'Lib\test'
if (Test-Path $libTest) { Remove-Item $libTest -Recurse -Force }
# Build artifacts that serve's headless path never reads (both gitignored in
# the checkout and usually absent, but a dev machine may have them).
foreach ($rel in @('hermes_cli\web_dist', 'hermes_cli\tui_dist')) {
  $p = Join-Path $appDir $rel
  if (Test-Path $p) { Remove-Item $p -Recurse -Force }
}
Get-ChildItem $outDir -Recurse -Directory -Filter '__pycache__' -ErrorAction SilentlyContinue |
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
# Entry-point script dir from the pip install — contains absolute-path shims
# we deliberately don't use (gateway.rs runs launch.py instead).
$scriptsDir = Join-Path $pythonDir 'Scripts'
if (Test-Path $scriptsDir) { Remove-Item $scriptsDir -Recurse -Force }

# --- 7. report ----------------------------------------------------------------
$size = (Get-ChildItem $outDir -Recurse -File | Measure-Object -Property Length -Sum).Sum
$sizeMb = [math]::Round($size / 1MB, 1)
Write-Host "[build-backend] done: $outDir ($sizeMb MB)"
Write-Host "[build-backend] verify with:"
Write-Host "  & '$stagingPython' '$(Join-Path $outDir 'launch.py')' --version"
