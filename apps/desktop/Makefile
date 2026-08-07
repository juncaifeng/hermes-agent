# Hermes Desktop (Tauri v2) — build helpers for Windows + Git Bash.
#
# Prerequisites:
#   - MinGW64 at E:\mingw64\bin (windres.exe — compiles Rust .rc resources;
#     without it tauri-winres panics with NotAttempted("windres"))
#   - WiX Toolset v3.14 at C:\Program Files (x86)\WiX Toolset v3.14 (MSI bundling)
#   - Node.js with repo-root npm install, Rust stable-x86_64-pc-windows-gnu
#
# Typical flow:
#   make -C apps/desktop msi      # produce the installer
#   make -C apps/desktop run      # bare-exe run with CDP debugging (uses staged dist)
#   make -C apps/desktop verify   # CDP e2e: loopback origin + gateway boot

MINGW_BIN := /e/mingw64/bin
WIX_BIN   := /c/Program Files (x86)/WiX Toolset v3.14/bin
WIX_DIR   := C:\Program Files (x86)\WiX Toolset v3.14

export PATH := $(MINGW_BIN):$(WIX_BIN):$(PATH)
export WIX  := $(WIX_DIR)

TAURI_DIR := src-tauri
RELEASE   := $(TAURI_DIR)/target/release
VERSION   := $(shell python -c "import json;print(json.load(open('$(TAURI_DIR)/tauri.conf.json'))['version'])" 2>/dev/null)
MSI       := $(RELEASE)/bundle/msi/Hermes_$(VERSION)_x64_en-US.msi

.PHONY: check build stage-dist exe msi run verify clean

check: ## cargo check, dev + release profiles (the release profile compiles prod-only cfg blocks)
	cd $(TAURI_DIR) && cargo check && cargo check --release

build: ## release build of hermes-desktop.exe
	cd $(TAURI_DIR) && cargo build --release

stage-dist: build ## copy renderer dist next to the exe — bare-exe layout matches the installed one
	rm -rf $(RELEASE)/dist
	cp -r dist $(RELEASE)/dist

exe: stage-dist ## exe + staged dist, runnable directly from target/release
	@echo "run: $(RELEASE)/hermes-desktop.exe"

msi: ## full MSI bundle via WiX (resources: dist/ lands at <install>/dist)
	## npm run build stages node-pty into dist/node_modules for the ELECTRON
	## build; Tauri refuses to bundle a frontendDist containing node_modules
	## (and it would bloat the resources copy), so strip it first.
	## NOTE: if this fails fast with "failed to build app", a hermes-desktop
	## instance is still holding the exe — close it (and stray WebView2
	## processes), wait a few seconds, and retry.
	rm -rf dist/node_modules
	npx tauri build --bundles msi
	@echo "MSI: $(MSI)"

run: exe ## launch the staged exe with CDP debugging on :9222
	cd $(RELEASE) && WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 ./hermes-desktop.exe &

verify: ## CDP e2e check (needs `make run` first)
	node e2e/check-loopback-origin.mjs

clean:
	cd $(TAURI_DIR) && cargo clean
