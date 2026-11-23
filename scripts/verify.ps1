$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $projectRoot
& npm.cmd run typecheck
if ($LASTEXITCODE -ne 0) { throw 'TypeScript check failed' }
& node --test scripts/test-diagnostics-ui.cjs scripts/test-gpu-ui.cjs scripts/test-model-ui.cjs scripts/test-microphone-ui.cjs scripts/test-dictionary-ui.cjs scripts/test-shortcut-ui.cjs scripts/test-polish-ui.cjs
if ($LASTEXITCODE -ne 0) { throw 'Diagnostics UI tests failed' }
& npm.cmd run build
if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
& cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
if ($LASTEXITCODE -ne 0) { throw 'Rust format check failed' }
& cargo test --manifest-path src-tauri/Cargo.toml
if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed' }
$python = Join-Path $projectRoot '.venv\Scripts\python.exe'
if (Test-Path -LiteralPath $python) {
    & $python -m pytest worker/tests
    if ($LASTEXITCODE -ne 0) { throw 'Worker tests failed' }
} else { Write-Output 'Worker tests skipped: .venv does not exist' }
