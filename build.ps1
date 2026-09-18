# TPT Bug Report Builder - build pipeline (Windows).
#
# Free edition (default):  ./build.ps1
#   cargo build --release --target wasm32-unknown-unknown
#   wasm-bindgen --target web  ->  web repo public/apps/bug-report-builder/ when
#   TPT_WEB_REPO is set (glue named bug-report-builder.js, wasm keeps its
#   _bg.wasm name - the glue resolves it via import.meta.url), otherwise
#   dist\web.
# Pro desktop edition:     ./build.ps1 -Pro
#   same, with --features pro (multi-screenshot sequences, scrolling
#   capture, Markdown/PDF report assembler, hotkeys compiled in), plus
#   trunk build --release --features pro and the native webview exe.
#
# Env:
#   TPT_WEB_REPO  path to the tptsolutions.co.nz web repo; the app is copied
#                 to <repo>\public\apps\bug-report-builder\ for the hub
#                 runner. Defaults to the known local checkout below if
#                 unset.

[CmdletBinding()]
param(
    # Build the Pro bundle (--features pro) instead of the free one.
    [switch]$Pro,
    # Only run the wasm-bindgen step against an existing release build.
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

$Crate = "tpt-app-bug-report-builder"
$Target = "wasm32-unknown-unknown"
$OutName = "bug-report-builder"
$Slug = "bug-report-builder"
$DefaultWebRepo = "D:\Programming\2 WIP\TPT Electrician\tpt-electrician-nz-2"

# Where the browser-loadable bundle goes.
$WebRepo = if ($env:TPT_WEB_REPO) { $env:TPT_WEB_REPO } elseif (Test-Path $DefaultWebRepo) { $DefaultWebRepo } else { $null }
$Dest = if ($WebRepo) {
    Join-Path $WebRepo "public\apps\bug-report-builder"
} else {
    Join-Path $PSScriptRoot "dist\web"
}

Write-Host "== TPT Bug Report Builder build ==" -ForegroundColor Cyan
Write-Host " edition: $(if ($Pro) { 'pro (multi-screenshot sequences + scrolling capture + Markdown/PDF + hotkeys)' } else { 'free (single screenshot)' })"
Write-Host " dest:    $Dest"

if (-not $SkipBuild) {
    $features = if ($Pro) { "--features", "pro" } else { @() }
    Write-Host "-- cargo build --release --target $Target" -ForegroundColor Cyan
    if ($features.Count -gt 0) {
        cargo build --release --target $Target -p $Crate @features
    } else {
        cargo build --release --target $Target -p $Crate
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$Wasm = Join-Path $PSScriptRoot "target\$Target\release\tpt_app_bug_report_builder.wasm"
if (-not (Test-Path $Wasm)) { throw "missing $Wasm - run the cargo build step first" }

Write-Host "-- wasm-bindgen --target web --out-name $OutName" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
wasm-bindgen --target web --out-name $OutName --out-dir $Dest $Wasm
if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed" }

$Glue = Join-Path $Dest "$OutName.js"
$BgWasm = Join-Path $Dest "$($OutName)_bg.wasm"
foreach ($artifact in @($Glue, $BgWasm)) {
    if (-not (Test-Path $artifact)) { throw "wasm-bindgen did not produce $artifact" }
}

# Payload size report. No hub-wide wasm size budget was found in apps.md or
# the runner (`src/components/tpt-apps/wasm-app-runner.tsx`) at the time this
# was written — fea-lite's ~2 MB gzip figure is that app's own FEM-solver
# budget, not a hub policy. 5 MB gzip is a generous local sanity check only,
# to catch an accidental regression (e.g. a debug build slipping through),
# not a real ceiling.
$raw = (Get-Item $BgWasm).Length
Add-Type -AssemblyName System.IO.Compression
$rawBytes = [System.IO.File]::ReadAllBytes($BgWasm)
$memoryStream = [System.IO.MemoryStream]::new()
$gzipStream = [System.IO.Compression.GZipStream]::new($memoryStream, [System.IO.Compression.CompressionLevel]::Optimal)
$gzipStream.Write($rawBytes, 0, $rawBytes.Length)
$gzipStream.Dispose()
$gzipped = $memoryStream.ToArray().Length
$memoryStream.Dispose()
$gzKb = [math]::Round($gzipped / 1KB, 1)
Write-Host "-- $(Split-Path $BgWasm -Leaf): $raw bytes raw, $gzipped bytes gzipped ($gzKb KB gz)" -ForegroundColor Cyan
if ($gzipped -gt 5MB) {
    Write-Warning "payload exceeds the 5 MB local sanity check (not a known hub budget)"
}

if ($Pro) {
    Write-Host "-- trunk build --release --features pro (standalone bundle for the desktop shell)" -ForegroundColor Cyan
    trunk build --release --features pro
    if ($LASTEXITCODE -ne 0) { throw "trunk build failed" }

    Write-Host "-- cargo build --release -p tpt-bugreport-desktop (webview exe)" -ForegroundColor Cyan
    cargo build --release -p tpt-bugreport-desktop
    if ($LASTEXITCODE -ne 0) { throw "desktop build failed" }
    Write-Host "   exe: target\release\tpt-bug-report-builder-pro.exe (serves dist\)" -ForegroundColor Cyan
}

Write-Host "-- done" -ForegroundColor Green
if (-not $WebRepo) {
    Write-Host "   (set TPT_WEB_REPO to copy the bundle into the hub web repo)"
} else {
    Write-Host ""
    Write-Host "Paste into src/lib/tpt-apps-registry.ts (TPT_APPS_REGISTRY), then flip status to 'live':" -ForegroundColor Cyan
    Write-Host @"
  {
    slug: '$Slug',
    name: 'TPT Bug Report Builder',
    description: 'Capture annotated screenshot sequences and auto-assemble a formatted bug report.',
    category: 'developer',
    status: 'coming-soon',
    wasm: { entry: '/apps/$Slug/$OutName.js' },
    // gumroadUrl: 'https://gum.co/...',  // add once the Pro exe is listed
  },
"@
}
