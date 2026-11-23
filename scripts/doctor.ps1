$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Write-Output "VoiceWeave workspace: $projectRoot"
Write-Output "OS: $([Environment]::OSVersion.VersionString)"
foreach ($command in @('node','npm.cmd','rustc','cargo','git','py')) {
    $item = Get-Command $command -ErrorAction SilentlyContinue
    if ($item) { Write-Output "$command : $($item.Source)" } else { Write-Output "$command : MISSING" }
}
Write-Output 'Python 3.12:'
& py -3.12 --version
if ($LASTEXITCODE -ne 0) { Write-Output 'Python 3.12 is required for the worker' }
Write-Output 'MSVC:'
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
if (Test-Path -LiteralPath $vswhere) {
    & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
} else { Write-Output 'VS Build Tools not detected' }
Write-Output 'GPU:'
if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    & nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader
} else { Write-Output 'NVIDIA GPU tool not found' }