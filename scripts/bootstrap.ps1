$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $projectRoot
$python = Join-Path $projectRoot '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $python)) {
    & py -3.12 -m venv (Join-Path $projectRoot '.venv')
    if ($LASTEXITCODE -ne 0) { throw 'Python 3.12 venv creation failed' }
}
$lock = Join-Path $projectRoot 'worker\requirements.lock'
if (Test-Path -LiteralPath $lock) {
    & $python -m pip install --requirement $lock
    if ($LASTEXITCODE -ne 0) { throw 'Worker locked dependencies failed' }
    & $python -m pip install --no-deps --editable (Join-Path $projectRoot 'worker')
} else {
    & $python -m pip install --editable './worker[stt,build,test]'
    if ($LASTEXITCODE -ne 0) { throw 'Initial worker dependencies failed' }
    & $python -m pip freeze --exclude-editable | Set-Content -LiteralPath $lock -Encoding utf8
}
if ($LASTEXITCODE -ne 0) { throw 'Worker package installation failed' }
& $python -m voiceweave_worker --self-check | & $python -c "import sys,struct,json; b=sys.stdin.buffer.read(); n=struct.unpack('<I',b[:4])[0]; h=json.loads(b[4:4+n]); assert h['kind']=='hello'; print(h)"
if ($LASTEXITCODE -ne 0) { throw 'Worker hello check failed' }