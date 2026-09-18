# TPT Bug Report Builder - task runner.
# The pipeline logic lives in build.ps1 (Windows) / build.sh (unix); these
# recipes are thin wrappers so `just` works everywhere.

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-Command"]

default:
    @just --list

# Build the free WASM edition and report the gzip payload size.
build:
    pwsh -NoLogo -NoProfile -File build.ps1

# Build the Pro edition: multi-screenshot sequences, scrolling capture,
# Markdown/PDF + hotkeys, the trunk bundle and the desktop exe.
pro:
    pwsh -NoLogo -NoProfile -File build.ps1 -Pro

# Re-run only wasm-bindgen + copy (after an existing release build).
bindgen:
    pwsh -NoLogo -NoProfile -File build.ps1 -SkipBuild

# Build the Pro edition and zip exe + dist into release/ for Gumroad.
package:
    pwsh -NoLogo -NoProfile -File build.ps1 -Pro -Package

# Engine validation suite (free + pro) plus a wasm type-check.
test:
    cargo test -p tpt-bugreport-engine
    cargo test -p tpt-bugreport-engine --features pro
    cargo check --target wasm32-unknown-unknown -p tpt-app-bug-report-builder

# Type-check the wasm lib (free + pro) and the desktop shell.
check:
    cargo check --target wasm32-unknown-unknown -p tpt-app-bug-report-builder
    cargo check --target wasm32-unknown-unknown --features pro -p tpt-app-bug-report-builder
    cargo check -p tpt-bugreport-desktop

# Live-reload standalone dev server (index.html mounts via #tpt-appfront-root).
dev:
    trunk serve

# Remove build outputs.
clean:
    cargo clean
    -rd /s /q dist 2>nul || rm -rf dist
