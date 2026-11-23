"""Keep the recognizer in memory while other programs crowd the PC (Windows only).

With little free memory, Windows moved most of the worker out to the page file
(3.3 GB private, 0.6 GB resident, 2026-10-11) and every recognition step then waited for
the disk: a 70-second recording took 10 to 12 seconds instead of under 2. After the
model is ready, the worker asks Windows to keep at least its current working set in
memory (a hard minimum). Best effort: if Windows refuses, nothing changes.
"""
from __future__ import annotations

import ctypes
import sys

FLOOR_BYTES = 512 * 1024 * 1024
CAP_BYTES = 2 * 1024 * 1024 * 1024
QUOTA_LIMITS_HARDWS_MIN_ENABLE = 0x1
QUOTA_LIMITS_HARDWS_MAX_DISABLE = 0x8
SE_PRIVILEGE_ENABLED = 0x2
TOKEN_ADJUST_PRIVILEGES = 0x20
TOKEN_QUERY = 0x8


def minimum_for(working_set: int) -> int:
    """The minimum to ask for: the current working set, within the floor and the cap."""
    return max(FLOOR_BYTES, min(working_set, CAP_BYTES))


def keep_resident() -> int | None:
    """Ask Windows to keep this process's working set in memory. Returns the minimum set
    in bytes, or None where it was refused or the platform has no such setting."""
    if sys.platform != "win32":
        return None
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)

    class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    class LUID(ctypes.Structure):
        _fields_ = [("LowPart", wintypes.DWORD), ("HighPart", wintypes.LONG)]

    class TOKEN_PRIVILEGES(ctypes.Structure):
        _fields_ = [
            ("PrivilegeCount", wintypes.DWORD),
            ("Luid", LUID),
            ("Attributes", wintypes.DWORD),
        ]

    # Declared argument types: the process handle is the pseudo-handle -1, which does not
    # fit ctypes' default int conversion.
    kernel32.GetCurrentProcess.restype = wintypes.HANDLE
    kernel32.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    advapi32.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    advapi32.LookupPrivilegeValueW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p]
    advapi32.AdjustTokenPrivileges.argtypes = [
        wintypes.HANDLE, wintypes.BOOL, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p, ctypes.c_void_p,
    ]
    process = kernel32.GetCurrentProcess()
    counters = PROCESS_MEMORY_COUNTERS()
    counters.cb = ctypes.sizeof(counters)
    if not kernel32.K32GetProcessMemoryInfo(process, ctypes.byref(counters), counters.cb):
        return None
    minimum = minimum_for(counters.WorkingSetSize)

    # Raising the minimum needs this privilege, which ordinary users hold but have off.
    token = wintypes.HANDLE()
    if advapi32.OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, ctypes.byref(token)):
        try:
            luid = LUID()
            if advapi32.LookupPrivilegeValueW(None, "SeIncreaseWorkingSetPrivilege", ctypes.byref(luid)):
                privileges = TOKEN_PRIVILEGES(1, luid, SE_PRIVILEGE_ENABLED)
                advapi32.AdjustTokenPrivileges(token, False, ctypes.byref(privileges), 0, None, None)
        finally:
            kernel32.CloseHandle(token)

    kernel32.SetProcessWorkingSetSizeEx.argtypes = [
        wintypes.HANDLE, ctypes.c_size_t, ctypes.c_size_t, wintypes.DWORD,
    ]
    ok = kernel32.SetProcessWorkingSetSizeEx(
        process,
        minimum,
        minimum + 256 * 1024 * 1024,
        QUOTA_LIMITS_HARDWS_MIN_ENABLE | QUOTA_LIMITS_HARDWS_MAX_DISABLE,
    )
    return minimum if ok else None
