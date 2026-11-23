# -*- mode: python ; coding: utf-8 -*-
import os

from PyInstaller.utils.hooks import collect_all, collect_data_files

datas = []
binaries = []
hiddenimports = []
tmp_ret = collect_all('faster_whisper')
datas += tmp_ret[0]; binaries += tmp_ret[1]; hiddenimports += tmp_ret[2]
tmp_ret = collect_all('ctranslate2')
datas += tmp_ret[0]; binaries += tmp_ret[1]; hiddenimports += tmp_ret[2]
# Parakeet (Japanese, and English for the English shortcut) runs on sherpa-onnx.
tmp_ret = collect_all('sherpa_onnx')
datas += tmp_ret[0]; binaries += tmp_ret[1]; hiddenimports += tmp_ret[2]
# The pinned model/CUDA catalogs are read with importlib.resources, and the installers
# and fast path are imported lazily, so PyInstaller cannot discover them by itself.
datas += collect_data_files('voiceweave_worker', includes=['*.json'])
hiddenimports += [
    'voiceweave_worker.cuda_install',
    'voiceweave_worker.model_install',
    'voiceweave_worker.short_window',
    'voiceweave_worker.streaming',
    'voiceweave_worker.resident',
    'voiceweave_worker.sherpa_engine',
    'voiceweave_worker.topics',
    'huggingface_hub',
]


a = Analysis(
    [os.path.join(SPECPATH, 'voiceweave_worker', '__main__.py')],
    pathex=[SPECPATH],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='voiceweave_worker',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=True,
    upx_exclude=[],
    name='voiceweave_worker',
)
