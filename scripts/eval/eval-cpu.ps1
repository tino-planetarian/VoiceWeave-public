# Measures recognition while speaking without a GPU (as on an office laptop with only
# integrated graphics), with synthetic speech: no microphone, no personal speech.
# For each model and each length: the worker's time per one-second step (it must stay
# under one second to keep up), the wait after stop, and the wait with it turned off.
#   -Cores 4   imitate a slower PC by using only 4 logical processors (0: all of this PC)
#   -Models    standard (turbo) and/or dev-tiny (development model folders)
#   -Lengths   sentences joined into one recording (8 = about 50 s, 26 = about 160 s)
# Synthetic audio and results go to -OutDir, outside the repository.
# Run only while the dev app is closed: it keeps the CPU busy for many minutes.
param([int]$Cores = 4, [string]$Models = 'standard,dev-tiny', [string]$Lengths = '3,8',
    [string]$OutDir)
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $OutDir) { $OutDir = Join-Path (Split-Path $projectRoot -Parent) 'voiceweave-eval-cpu' }
if (Get-Process voiceweave, VoiceWeave -ErrorAction SilentlyContinue) { throw 'VoiceWeave is running. Quit it first.' }
$python = Join-Path $projectRoot '.venv\Scripts\python.exe'
$audio = Join-Path $OutDir 'audio'
if (-not (Test-Path (Join-Path $audio 'normal_a26.wav'))) {
    & (Join-Path $PSScriptRoot 'make-asr-audio.ps1') -OutDir $audio -Tsv astro_eval_ja.tsv -Only normal
}
$results = Join-Path $OutDir ("cpu-{0}cores-{1}.jsonl" -f $Cores, (Get-Date -Format 'yyyyMMdd-HHmm'))
foreach ($model in $Models -split ',') {
    $dir = Join-Path $projectRoot "models\$model"
    $revision = Get-ChildItem -Directory $dir -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($revision -and -not (Test-Path (Join-Path $dir 'model.bin'))) { $dir = $revision.FullName }
    if (-not (Test-Path (Join-Path $dir 'model.bin'))) { throw "No model.bin under $dir" }
    foreach ($length in $Lengths -split ',') {
        Write-Output "== $model, $length sentences, $Cores logical processors"
        & $python (Join-Path $PSScriptRoot 'eval-worker-stream.py') --audio-dir $audio --model-dir $dir `
            --device cpu --threads 4 --cores $Cores --modes none,off --sentences $length |
            ForEach-Object { $_; ('{{"model":"{0}","sentences":{1},"cores":{2},"result":{3}}}' -f $model, $length, $Cores, $_) |
                Add-Content -Encoding utf8 $results }
        if ($LASTEXITCODE -ne 0) { throw "eval-worker-stream.py failed ($model, $length)" }
    }
}
Write-Output "Saved: $results"
