# Installs the NSIS package silently into a test folder, checks the installed files and
# the bundled worker, then uninstalls and checks the cleanup. The app itself is not
# started (it would register global shortcuts). No microphone, model or network use.
# Refuses to run when VoiceWeave is already installed or running, so a real
# installation is never touched.
param(
    [string]$Installer = (Join-Path $PSScriptRoot '..\src-tauri\target\release\bundle\nsis\VoiceWeave_0.1.0_x64-setup.exe'),
    [string]$InstallDir = 'G:\Codexアプリ開発\cuda-probe-tmp\install-smoke'
)
$ErrorActionPreference = 'Stop'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\VoiceWeave'
$results = [System.Collections.Generic.List[string]]::new()
function Check([string]$name, [bool]$ok) {
    $results.Add(('{0}  {1}' -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $name))
}

$Installer = (Resolve-Path $Installer).Path
if (Get-Process VoiceWeave, voiceweave, voiceweave_worker -ErrorAction SilentlyContinue) {
    throw 'VoiceWeave is running. Quit it first.'
}
if (Test-Path $uninstallKey) {
    throw 'VoiceWeave is already installed for this user; this test will not touch it.'
}
if (Test-Path $InstallDir) { throw "Test folder already exists: $InstallDir" }

$started = Get-Date
# NSIS: /S silent; /D= must be last and unquoted.
$setup = Start-Process -FilePath $Installer -ArgumentList '/S', "/D=$InstallDir" -PassThru -Wait
Check "installer exit code 0 ($($setup.ExitCode))" ($setup.ExitCode -eq 0)
Check 'uninstall entry registered' (Test-Path $uninstallKey)

$mainExe = @('VoiceWeave.exe', 'voiceweave.exe') | ForEach-Object { Join-Path $InstallDir $_ } |
    Where-Object { Test-Path $_ } | Select-Object -First 1
Check "main executable present ($([IO.Path]::GetFileName([string]$mainExe)))" ([bool]$mainExe)
$worker = Join-Path $InstallDir 'stt\voiceweave_worker\voiceweave_worker.exe'
$internal = Join-Path $InstallDir 'stt\voiceweave_worker\_internal'
Check 'bundled worker executable' (Test-Path $worker)
Check 'Silero VAD model' (Test-Path (Join-Path $internal 'faster_whisper\assets\silero_vad_v6.onnx'))
Check 'model catalog' (Test-Path (Join-Path $internal 'voiceweave_worker\model_catalog.json'))
Check 'CUDA catalog' (Test-Path (Join-Path $internal 'voiceweave_worker\cuda_catalog.json'))
Check 'CTranslate2 native library' ([bool](Get-ChildItem $internal -Recurse -Filter 'ctranslate2*.dll' -ErrorAction SilentlyContinue))
$size = (Get-ChildItem $InstallDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
$results.Add(('INFO  installed size {0:N0} MB, took {1:N0} s' -f $size, ((Get-Date) - $started).TotalSeconds))

if (Test-Path $worker) {
    # The worker answers --self-check with one hello frame (length-prefixed JSON).
    $psi = New-Object Diagnostics.ProcessStartInfo $worker
    $psi.Arguments = '--self-check'
    $psi.RedirectStandardOutput = $true; $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($psi)
    $stream = New-Object IO.MemoryStream
    $process.StandardOutput.BaseStream.CopyTo($stream)
    $process.WaitForExit()
    $bytes = $stream.ToArray()
    $json = if ($bytes.Length -gt 4) { [Text.Encoding]::UTF8.GetString($bytes, 4, $bytes.Length - 4) } else { '' }
    Check 'installed worker self-check (hello frame, exit 0)' (($process.ExitCode -eq 0) -and ($json -match '"kind"\s*:\s*"hello"'))
    $help = & $worker --model-install --help 2>&1 | Out-String
    Check 'installed worker dispatches --model-install' ($LASTEXITCODE -eq 0 -and $help -match 'model-root')
    $help = & $worker --cuda-install --help 2>&1 | Out-String
    Check 'installed worker dispatches --cuda-install' ($LASTEXITCODE -eq 0 -and $help -match 'root')
}

$uninstaller = Get-ChildItem $InstallDir -Filter 'uninstall*.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
Check 'uninstaller present' ([bool]$uninstaller)
if ($uninstaller) {
    # _?= keeps the uninstaller from copying itself away, so -Wait really waits.
    $remove = Start-Process -FilePath $uninstaller.FullName -ArgumentList '/S', "_?=$InstallDir" -PassThru -Wait
    Check "uninstaller exit code 0 ($($remove.ExitCode))" ($remove.ExitCode -eq 0)
    # With _?= the uninstaller itself stays; remove it and the emptied folder.
    Remove-Item -LiteralPath $uninstaller.FullName -Force -ErrorAction SilentlyContinue
    $left = @(Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue)
    Check "installed files removed ($($left.Count) left)" ($left.Count -eq 0)
    if ($left.Count -eq 0) { Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue }
    Check 'uninstall entry removed' (-not (Test-Path $uninstallKey))
}
$results
if ($results -match '^FAIL') { exit 1 }
