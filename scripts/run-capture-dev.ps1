# Starts the development capture build for a joint microphone test.
# -Cuda <dir>  directory holding cublas64_12.dll and cublasLt64_12.dll (enables GPU)
# -Cpu         force CPU for comparison
# -Cores <n>   run on only the first n logical processors, to imitate a slower PC
#              (with -Cpu and -Cores 4: roughly an older office laptop)
# Refuses to start while another VoiceWeave is running. Quit it from the tray first.
param([string]$Cuda, [switch]$Cpu, [int]$Cores = 0)
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = Join-Path $projectRoot 'src-tauri\target\capture-dev\debug\voiceweave.exe'
if (-not (Test-Path $exe)) { throw 'Build first: scripts/build-capture-dev.ps1' }
if (Get-Process voiceweave, VoiceWeave -ErrorAction SilentlyContinue) { throw 'VoiceWeave is already running. Quit it from the tray first.' }

# The debug build loads the UI from the Vite preview of the latest dist on port 1420.
if (-not (Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue)) {
    Start-Process -WindowStyle Hidden -WorkingDirectory $projectRoot -FilePath 'npm.cmd' `
        -ArgumentList 'exec','--','vite','preview','--host','127.0.0.1','--port','1420','--strictPort'
}
# The hidden recording overlay loads its page once at startup. If the preview is not
# answering yet, the transparent overlay stays empty (invisible) for the whole session.
$deadline = (Get-Date).AddSeconds(30)
while ($true) {
    try {
        if ((Invoke-WebRequest 'http://127.0.0.1:1420/' -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200) { break }
    } catch { }
    if ((Get-Date) -gt $deadline) { throw 'The UI preview on 127.0.0.1:1420 did not answer within 30 s' }
    Start-Sleep -Milliseconds 300
}

$env:VOICEWEAVE_PYTHON = (Resolve-Path (Join-Path $projectRoot '.venv\Scripts\python.exe')).Path
foreach ($name in 'VOICEWEAVE_TEST_INPUT', 'VOICEWEAVE_TEST_INPUT_DELAY_MS', 'VOICEWEAVE_MODEL_DIR', 'VOICEWEAVE_CUDA_DIR', 'VOICEWEAVE_DEVICE') {
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}
if ($Cpu) { $env:VOICEWEAVE_DEVICE = 'cpu' }
elseif ($Cuda) { $env:VOICEWEAVE_CUDA_DIR = (Resolve-Path $Cuda).Path }
if ($Cores -gt 0) {
    # Processes started from here inherit it: the app and its recognition worker.
    [System.Diagnostics.Process]::GetCurrentProcess().ProcessorAffinity = [IntPtr]((1L -shl $Cores) - 1)
}
Start-Process -FilePath $exe -WorkingDirectory $projectRoot
Write-Output 'Started. Wait for the "prepared" notice / settings screen to show the device before pressing F8.'
