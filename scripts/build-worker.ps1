$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $projectRoot
$python = Join-Path $projectRoot '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $python)) { throw 'Run scripts/bootstrap.ps1 first' }
& $python -m PyInstaller --noconfirm --clean --distpath worker/dist --workpath worker/build worker/voiceweave-worker.spec
if ($LASTEXITCODE -ne 0) { throw 'PyInstaller build failed' }
$from = (Resolve-Path -LiteralPath 'worker/dist/voiceweave_worker').Path
$to = Join-Path $projectRoot 'src-tauri/resources/stt/voiceweave_worker'
$resourceRoot = Join-Path $projectRoot 'src-tauri/resources/stt'
New-Item -ItemType Directory -Path $resourceRoot -Force | Out-Null
if (Test-Path -LiteralPath $to) {
    $resolved = (Resolve-Path -LiteralPath $to).Path
    if (-not $resolved.StartsWith($resourceRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe resource path' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
Copy-Item -LiteralPath $from -Destination $to -Recurse
Write-Output "Worker resources: $to"