# Synthesizes the evaluation sentences with the Windows Japanese voice (no microphone,
# no personal speech). Output goes to -OutDir, which must be outside the repository.
param([Parameter(Mandatory)][string]$OutDir, [int[]]$Rates = @(0, 5))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$sentences = Get-Content -Encoding utf8 (Join-Path $PSScriptRoot 'sentences_ja.txt') | Where-Object { $_.Trim() }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000,
    [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
foreach ($rate in $Rates) {
    for ($i = 0; $i -lt $sentences.Count; $i++) {
        $voice = New-Object System.Speech.Synthesis.SpeechSynthesizer
        $voice.SelectVoice('Microsoft Haruka Desktop')
        $voice.Rate = $rate
        $voice.SetOutputToWaveFile((Join-Path $OutDir ('r{0}_{1:D2}.wav' -f $rate, $i)), $format)
        $voice.Speak($sentences[$i])
        $voice.Dispose()
    }
}
Write-Output "$($sentences.Count * $Rates.Count) files in $OutDir"
