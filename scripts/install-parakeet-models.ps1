# Copies the Parakeet models (Japanese, and English for the English shortcut) from the
# development folder to the installed VoiceWeave's own model folder, so the installed
# app can use them. Run this in your own PowerShell window with VoiceWeave closed.
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-parakeet-models.ps1
# The destination is %LOCALAPPDATA%\local.voiceweave.app\models\<name>, where the app
# looks for its models (the same place as the standard and accurate presets).
param(
    [string]$Source = (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..')).Path 'models'),
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'local.voiceweave.app\models')
)
$ErrorActionPreference = 'Stop'
$running = Get-Process -Name 'VoiceWeave' -ErrorAction SilentlyContinue
if ($running) { throw 'VoiceWeave is running. Close it (also from the tray) and run this again.' }
$wanted = @{
    'parakeet-ja' = @('model.int8.onnx', 'tokens.txt')
    'parakeet-en' = @('encoder.int8.onnx', 'decoder.int8.onnx', 'joiner.int8.onnx', 'tokens.txt')
}
foreach ($name in $wanted.Keys) {
    $from = Join-Path $Source $name
    foreach ($file in $wanted[$name]) {
        if (-not (Test-Path -LiteralPath (Join-Path $from $file))) { throw "Missing: $from\$file" }
    }
    $to = Join-Path $Destination $name
    New-Item -ItemType Directory -Path $to -Force | Out-Null
    foreach ($file in $wanted[$name]) {
        Copy-Item -LiteralPath (Join-Path $from $file) -Destination (Join-Path $to $file) -Force
    }
    Write-Output "Copied $name -> $to"
}
Write-Output 'Done. Start VoiceWeave, then pick Parakeet in the recognition preset list (basic tab) and save.'
