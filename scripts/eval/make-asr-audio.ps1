# Synthesizes asr_eval_ja.tsv with the Windows Japanese voice (no microphone, no personal
# speech) for scripts/eval/eval-asr.py. Output goes to -OutDir, outside the repository.
# Three conditions: normal speed, fast, and fast with the punctuation removed from the
# input so the voice makes no pauses (the case where punctuation must come from meaning).
# -Tsv picks another sentence file (astro_eval_ja.tsv); an optional third column is what the
# voice reads when it would misread the written form (a one-kanji term read the other way).
# -Only limits the conditions made (normal,fast,nopause).
param([Parameter(Mandatory)][string]$OutDir, [string]$Tsv = 'asr_eval_ja.tsv',
    [string]$Only = 'normal,fast,nopause')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$rows = Get-Content -Encoding utf8 (Join-Path $PSScriptRoot $Tsv) | Where-Object { $_.Trim() }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000,
    [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$conditions = @(@{ Name = 'normal'; Rate = 0; Pauses = $true }, @{ Name = 'fast'; Rate = 5; Pauses = $true },
    @{ Name = 'nopause'; Rate = 5; Pauses = $false }) | Where-Object { ($Only -split ',') -contains $_.Name }
foreach ($condition in $conditions) {
    foreach ($row in $rows) {
        $parts = $row -split "`t"
        $id, $text = $parts[0], $parts[1]
        if ($parts.Count -gt 2 -and $parts[2]) { $text = $parts[2] }
        # Comma and full stop by code point: PowerShell 5.1 reads this file as ANSI.
        if (-not $condition.Pauses) { $text = $text -replace "[$([char]0x3001)$([char]0x3002)]", '' }
        $voice = New-Object System.Speech.Synthesis.SpeechSynthesizer
        $voice.SelectVoice('Microsoft Haruka Desktop')
        $voice.Rate = $condition.Rate
        $voice.SetOutputToWaveFile((Join-Path $OutDir "$($condition.Name)_$id.wav"), $format)
        $voice.Speak($text)
        $voice.Dispose()
    }
}
Write-Output "$($rows.Count * $conditions.Count) files in $OutDir"
