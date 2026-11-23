$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetRoot = Join-Path $projectRoot 'src-tauri\target\capture-dev'
$previousJobs = $env:CARGO_BUILD_JOBS
try {
    $env:CARGO_BUILD_JOBS = '1'
    & cargo build --offline --manifest-path (Join-Path $projectRoot 'src-tauri\Cargo.toml') `
        --features experimental-audio --bin voiceweave --target-dir $targetRoot
    if ($LASTEXITCODE -ne 0) { throw 'Capture development build failed' }
    Write-Output (Join-Path $targetRoot 'debug\voiceweave.exe')
} finally {
    $env:CARGO_BUILD_JOBS = $previousJobs
}
