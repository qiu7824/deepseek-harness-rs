"""Install the unmodified production Flutter installer and verify GUI bootstrap.

Windows only; standard-library only. This exercises installed GUI -> real Host,
not all UI interactions. Every launch enters a private kill-on-close Job before
its first instruction runs. Existing installations or desktop instances cause a
refusal, never a process-name/PID kill. Results and logs remain in --workdir.
"""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes as w
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.request
import uuid
import zipfile

APP_ID = "{37BA446F-D181-493B-9703-23978B3C194A}_is1"
R8_REVISION = "da1c20992f9d66d84ee179cf1c8b00d997729df1"
R6_REVISION = "58498fbaf12c476a0d54fbc3b98848440bd01cf0"


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_json(path: Path) -> dict:
    if path.stat().st_size > 128 * 1024:
        raise ValueError(f"JSON exceeds diagnostic limit: {path}")
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected JSON object: {path}")
    return value


def same_path(left: str | Path, right: str | Path) -> bool:
    def normalize(value):
        text = str(value)
        if text.startswith("\\\\?\\UNC\\"):
            text = "\\\\" + text[8:]
        elif text.startswith("\\\\?\\"):
            text = text[4:]
        return os.path.normcase(os.path.realpath(text))
    return normalize(left) == normalize(right)


def log_tail(path: Path, limit=128 * 1024) -> str:
    with path.open("rb") as stream:
        stream.seek(max(0, path.stat().st_size - limit))
        return stream.read(limit).decode("utf-8", errors="replace")


def rpc(address: str, method: str = "host.describe") -> dict:
    parsed = urllib.parse.urlsplit(address)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port:
        raise ValueError("Readiness address must be an IPv4 loopback HTTP origin")
    if parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ("", "/"):
        raise ValueError("Readiness address is not an origin")
    request_id = str(uuid.uuid4())
    body = json.dumps({"type": "client-request", "rpcId": request_id,
                       "method": method, "payload": {}}).encode()
    request = urllib.request.Request(address.rstrip("/") + "/api/" + method,
        data=body, headers={"Content-Type": "application/json", "Origin": address,
                            "Sec-Fetch-Site": "same-origin"})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=3) as response:
        if response.url != request.full_url:
            raise ValueError("Host RPC redirected outside the requested local endpoint")
        raw = response.read(65537)
    if len(raw) > 65536:
        raise ValueError("Host response exceeds diagnostic limit")
    value = json.loads(raw)
    if value.get("rpcId") != request_id or value.get("result", {}).get("ok") is not True:
        raise ValueError("Host RPC did not return the requested successful result")
    return value["result"]["value"]


class STARTUPINFO(ctypes.Structure):
    _fields_ = [("cb", w.DWORD), ("lpReserved", w.LPWSTR), ("lpDesktop", w.LPWSTR),
        ("lpTitle", w.LPWSTR), ("dwX", w.DWORD), ("dwY", w.DWORD),
        ("dwXSize", w.DWORD), ("dwYSize", w.DWORD), ("dwXCountChars", w.DWORD),
        ("dwYCountChars", w.DWORD), ("dwFillAttribute", w.DWORD),
        ("dwFlags", w.DWORD), ("wShowWindow", w.WORD), ("cbReserved2", w.WORD),
        ("lpReserved2", ctypes.POINTER(ctypes.c_byte)), ("hStdInput", w.HANDLE),
        ("hStdOutput", w.HANDLE), ("hStdError", w.HANDLE)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", w.HANDLE), ("hThread", w.HANDLE),
                ("dwProcessId", w.DWORD), ("dwThreadId", w.DWORD)]


class PROCESSENTRY32(ctypes.Structure):
    _fields_ = [("dwSize", w.DWORD), ("cntUsage", w.DWORD), ("th32ProcessID", w.DWORD),
        ("th32DefaultHeapID", ctypes.c_size_t), ("th32ModuleID", w.DWORD),
        ("cntThreads", w.DWORD), ("th32ParentProcessID", w.DWORD),
        ("pcPriClassBase", w.LONG), ("dwFlags", w.DWORD), ("szExeFile", w.WCHAR * 260)]


class JOB_BASIC_LIMIT(ctypes.Structure):
    _fields_ = [("PerProcessUserTimeLimit", ctypes.c_longlong),
        ("PerJobUserTimeLimit", ctypes.c_longlong), ("LimitFlags", w.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t), ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", w.DWORD), ("Affinity", ctypes.c_size_t),
        ("PriorityClass", w.DWORD), ("SchedulingClass", w.DWORD)]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]


class JOB_EXTENDED_LIMIT(ctypes.Structure):
    _fields_ = [("BasicLimitInformation", JOB_BASIC_LIMIT), ("IoInfo", IO_COUNTERS),
        ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]


class SID_AND_ATTRIBUTES(ctypes.Structure):
    _fields_ = [("Sid", ctypes.c_void_p), ("Attributes", w.DWORD)]


class TOKEN_GROUPS(ctypes.Structure):
    _fields_ = [("GroupCount", w.DWORD), ("Groups", SID_AND_ATTRIBUTES * 1)]


class JOB_ACCOUNTING(ctypes.Structure):
    _fields_ = [("TotalUserTime", ctypes.c_longlong), ("TotalKernelTime", ctypes.c_longlong),
        ("ThisPeriodTotalUserTime", ctypes.c_longlong), ("ThisPeriodTotalKernelTime", ctypes.c_longlong),
        ("TotalPageFaultCount", w.DWORD), ("TotalProcesses", w.DWORD),
        ("ActiveProcesses", w.DWORD), ("TotalTerminatedProcesses", w.DWORD)]


class Windows:
    def __init__(self):
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.user = ctypes.WinDLL("user32", use_last_error=True)
        self.advapi = ctypes.WinDLL("advapi32", use_last_error=True)
        self._bind(self.kernel, "CreateJobObjectW", w.HANDLE, [ctypes.c_void_p, w.LPCWSTR])
        self._bind(self.kernel, "SetInformationJobObject", w.BOOL, [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD])
        self._bind(self.kernel, "AssignProcessToJobObject", w.BOOL, [w.HANDLE, w.HANDLE])
        self._bind(self.kernel, "TerminateJobObject", w.BOOL, [w.HANDLE, w.UINT])
        self._bind(self.kernel, "IsProcessInJob", w.BOOL, [w.HANDLE, w.HANDLE, ctypes.POINTER(w.BOOL)])
        self._bind(self.kernel, "QueryInformationJobObject", w.BOOL, [w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD, ctypes.c_void_p])
        self._bind(self.kernel, "CreateProcessW", w.BOOL, [w.LPCWSTR, w.LPWSTR,
            ctypes.c_void_p, ctypes.c_void_p, w.BOOL, w.DWORD, ctypes.c_void_p,
            w.LPCWSTR, ctypes.POINTER(STARTUPINFO), ctypes.POINTER(PROCESS_INFORMATION)])
        self._bind(self.kernel, "CloseHandle", w.BOOL, [w.HANDLE])
        self._bind(self.kernel, "ResumeThread", w.DWORD, [w.HANDLE])
        self._bind(self.kernel, "WaitForSingleObject", w.DWORD, [w.HANDLE, w.DWORD])
        self._bind(self.kernel, "GetExitCodeProcess", w.BOOL, [w.HANDLE, ctypes.POINTER(w.DWORD)])
        self._bind(self.kernel, "TerminateProcess", w.BOOL, [w.HANDLE, w.UINT])
        self._bind(self.kernel, "OpenProcess", w.HANDLE, [w.DWORD, w.BOOL, w.DWORD])
        self._bind(self.kernel, "QueryFullProcessImageNameW", w.BOOL, [w.HANDLE, w.DWORD, w.LPWSTR, ctypes.POINTER(w.DWORD)])
        self._bind(self.kernel, "GetProcessTimes", w.BOOL, [w.HANDLE, ctypes.POINTER(w.FILETIME),
            ctypes.POINTER(w.FILETIME), ctypes.POINTER(w.FILETIME), ctypes.POINTER(w.FILETIME)])
        self._bind(self.kernel, "CreateToolhelp32Snapshot", w.HANDLE, [w.DWORD, w.DWORD])
        self._bind(self.kernel, "Process32FirstW", w.BOOL, [w.HANDLE, ctypes.POINTER(PROCESSENTRY32)])
        self._bind(self.kernel, "Process32NextW", w.BOOL, [w.HANDLE, ctypes.POINTER(PROCESSENTRY32)])
        self._bind(self.kernel, "GetCurrentProcess", w.HANDLE, [])
        self._bind(self.kernel, "OpenMutexW", w.HANDLE, [w.DWORD, w.BOOL, w.LPCWSTR])
        self._bind(self.kernel, "LocalFree", ctypes.c_void_p, [ctypes.c_void_p])
        self._bind(self.advapi, "OpenProcessToken", w.BOOL, [w.HANDLE, w.DWORD, ctypes.POINTER(w.HANDLE)])
        self._bind(self.advapi, "GetTokenInformation", w.BOOL, [w.HANDLE, ctypes.c_int,
            ctypes.c_void_p, w.DWORD, ctypes.POINTER(w.DWORD)])
        self._bind(self.advapi, "ConvertSidToStringSidW", w.BOOL, [ctypes.c_void_p, ctypes.POINTER(w.LPWSTR)])
        self._bind(self.user, "FindWindowW", w.HWND, [w.LPCWSTR, w.LPCWSTR])
        self._bind(self.user, "IsWindowVisible", w.BOOL, [w.HWND])
        self._bind(self.user, "GetWindowThreadProcessId", w.DWORD, [w.HWND, ctypes.POINTER(w.DWORD)])
        self._bind(self.user, "PostMessageW", w.BOOL, [w.HWND, w.UINT, w.WPARAM, w.LPARAM])
        self.enum_callback = ctypes.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        self._bind(self.user, "EnumWindows", w.BOOL, [self.enum_callback, w.LPARAM])

    @staticmethod
    def _bind(library, name, result, arguments):
        fn = getattr(library, name)
        fn.restype, fn.argtypes = result, arguments

    @staticmethod
    def check(value, operation):
        if not value:
            raise ctypes.WinError(ctypes.get_last_error(), operation)
        return value

    def identity(self, handle) -> dict:
        buffer, size = ctypes.create_unicode_buffer(32768), w.DWORD(32768)
        self.check(self.kernel.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)), "query image")
        created, exited, kernel, user = [w.FILETIME() for _ in range(4)]
        self.check(self.kernel.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited),
            ctypes.byref(kernel), ctypes.byref(user)), "query creation time")
        return {"executable": buffer.value, "creationTime": (created.dwHighDateTime << 32) | created.dwLowDateTime}

    def processes(self) -> dict[int, int]:
        handle = self.kernel.CreateToolhelp32Snapshot(2, 0)
        if handle == ctypes.c_void_p(-1).value:
            self.check(False, "process snapshot")
        try:
            entry = PROCESSENTRY32()
            entry.dwSize = ctypes.sizeof(entry)
            self.check(self.kernel.Process32FirstW(handle, ctypes.byref(entry)), "first process")
            result = {}
            while True:
                result[entry.th32ProcessID] = entry.th32ParentProcessID
                if not self.kernel.Process32NextW(handle, ctypes.byref(entry)):
                    break
            return result
        finally:
            self.kernel.CloseHandle(handle)

    def windows(self, pid: int) -> list[int]:
        result = []
        @self.enum_callback
        def callback(window, unused):
            owner = w.DWORD()
            self.user.GetWindowThreadProcessId(window, ctypes.byref(owner))
            if owner.value == pid and self.user.IsWindowVisible(window):
                result.append(int(window))
            return True
        self.check(self.user.EnumWindows(callback, 0), "enumerate windows")
        return result

    def token(self) -> dict:
        token = w.HANDLE()
        self.check(self.advapi.OpenProcessToken(self.kernel.GetCurrentProcess(), 8, ctypes.byref(token)), "open own token")
        try:
            length, elevated, elevation_type = w.DWORD(), w.DWORD(), w.DWORD()
            self.check(self.advapi.GetTokenInformation(token, 20, ctypes.byref(elevated), 4,
                ctypes.byref(length)), "token elevation")
            self.check(self.advapi.GetTokenInformation(token, 18, ctypes.byref(elevation_type), 4,
                ctypes.byref(length)), "token elevation type")
            self.advapi.GetTokenInformation(token, 2, None, 0, ctypes.byref(length))
            groups = ctypes.create_string_buffer(length.value)
            self.check(self.advapi.GetTokenInformation(token, 2, groups, len(groups), ctypes.byref(length)), "token groups")
            count = w.DWORD.from_buffer(groups).value
            if count > 1024:
                raise ValueError("Unexpected token group count")
            administrator = False
            for index in range(count):
                group = SID_AND_ATTRIBUTES.from_buffer(groups, TOKEN_GROUPS.Groups.offset + index * ctypes.sizeof(SID_AND_ATTRIBUTES))
                value = w.LPWSTR()
                self.check(self.advapi.ConvertSidToStringSidW(group.Sid, ctypes.byref(value)), "token group SID")
                try:
                    administrator |= value.value == "S-1-5-32-544"
                finally:
                    self.kernel.LocalFree(value)
            # Include deny-only admin membership: a UAC-filtered administrator
            # is not a standard-user account even when elevation=false.
            standard = not elevated.value and elevation_type.value == 1 and not administrator
            return {"elevated": bool(elevated.value), "elevationType": elevation_type.value,
                    "administratorGroupPresent": administrator, "standardUser": standard}
        finally:
            self.kernel.CloseHandle(token)


class Process:
    """An original process handle and its private descendant Job Object."""
    def __init__(self, api: Windows, executable: Path, arguments: list[str], cwd: Path, environment: dict, output: Path | None = None):
        self.api, self.handle, self.job, self.pid = api, None, None, 0
        self.children = {}
        job = api.check(api.kernel.CreateJobObjectW(None, None), "create private process job")
        self.job = job
        limits = JOB_EXTENDED_LIMIT()
        limits.BasicLimitInformation.LimitFlags = 0x2000  # KILL_ON_JOB_CLOSE
        info, startup = PROCESS_INFORMATION(), STARTUPINFO()
        startup.cb = ctypes.sizeof(startup)
        inherited = []
        if output is not None:
            import msvcrt
            inherited = [output.open("ab", buffering=0), open(os.devnull, "rb", buffering=0)]
            for stream in inherited:
                os.set_handle_inheritable(msvcrt.get_osfhandle(stream.fileno()), True)
            startup.dwFlags = 0x100
            startup.hStdOutput = startup.hStdError = msvcrt.get_osfhandle(inherited[0].fileno())
            startup.hStdInput = msvcrt.get_osfhandle(inherited[1].fileno())
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(executable), *arguments]))
        block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(environment.items(), key=lambda x: x[0].upper())) + "\0\0")
        try:
            api.check(api.kernel.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits)), "set private job limits")
            api.check(api.kernel.CreateProcessW(str(executable), command, None, None, bool(inherited),
                4 | 0x400 | 0x08000000, block, str(cwd), ctypes.byref(startup), ctypes.byref(info)), "create suspended fixture process")
            self.handle, self.pid = info.hProcess, info.dwProcessId
            api.check(api.kernel.AssignProcessToJobObject(job, self.handle), "assign fixture job before execution")
            self.identity = api.identity(self.handle)
            if api.kernel.ResumeThread(info.hThread) == 0xFFFFFFFF:
                api.check(False, "resume fixture process")
        except BaseException:
            if self.handle:
                api.kernel.TerminateProcess(self.handle, 1)  # Original suspended handle only.
            self.close()
            raise
        finally:
            for stream in inherited:
                stream.close()
            if info.hThread:
                api.kernel.CloseHandle(info.hThread)

    def alive(self):
        return self.api.kernel.WaitForSingleObject(self.handle, 0) == 0x102

    def wait(self, seconds):
        status = self.api.kernel.WaitForSingleObject(self.handle, int(seconds * 1000))
        if status == 0x102:
            raise TimeoutError(f"Fixture process {self.pid} did not exit")
        if status != 0:
            self.api.check(False, "wait fixture process")
        code = w.DWORD()
        self.api.check(self.api.kernel.GetExitCodeProcess(self.handle, ctypes.byref(code)), "fixture exit code")
        return code.value

    def owned_host(self, pid: int, expected: Path) -> dict:
        handle = self.api.check(self.api.kernel.OpenProcess(0x1000 | 0x100000, False, pid), "open reported Host for inspection")
        try:
            identity = self.api.identity(handle)
            inside = w.BOOL()
            self.api.check(self.api.kernel.IsProcessInJob(handle, self.job, ctypes.byref(inside)), "Host job membership")
            parent = self.api.processes().get(pid)
            if not inside.value or parent != self.pid or not same_path(identity["executable"], expected):
                raise ValueError("Reported Host is not a direct child in the GUI's private job")
            if identity["creationTime"] < self.identity["creationTime"]:
                raise ValueError("Reported Host predates its GUI")
            if self.api.kernel.WaitForSingleObject(handle, 0) != 0x102:
                raise ValueError("Reported Host is no longer alive")
            return {"pid": pid, "parentPid": parent, "privateJobMember": True, **identity}
        finally:
            self.api.kernel.CloseHandle(handle)

    def track_hosts(self, expected: Path):
        for pid, parent in self.api.processes().items():
            if parent != self.pid or pid in self.children:
                continue
            handle = self.api.kernel.OpenProcess(0x1000 | 0x100000, False, pid)
            if not handle:
                continue  # It may have exited before observation; never claim ownership.
            try:
                identity = self.api.identity(handle)
                inside = w.BOOL()
                self.api.check(self.api.kernel.IsProcessInJob(handle, self.job, ctypes.byref(inside)), "observed child job")
                if inside.value and same_path(identity["executable"], expected) and identity["creationTime"] >= self.identity["creationTime"]:
                    self.children[pid] = (handle, {"pid": pid, "parentPid": parent, **identity})
                    handle = None
            finally:
                if handle:
                    self.api.kernel.CloseHandle(handle)

    def child_reports(self):
        result = []
        for handle, identity in self.children.values():
            report = dict(identity)
            if self.api.kernel.WaitForSingleObject(handle, 0) == 0:
                code = w.DWORD()
                self.api.check(self.api.kernel.GetExitCodeProcess(handle, ctypes.byref(code)), "observed child exit code")
                report["exitCode"] = code.value
            result.append(report)
        return result

    def wait_job_idle(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            accounting = JOB_ACCOUNTING()
            self.api.check(self.api.kernel.QueryInformationJobObject(self.job, 1, ctypes.byref(accounting),
                ctypes.sizeof(accounting), None), "fixture job accounting")
            if not accounting.ActiveProcesses:
                return
            time.sleep(.1)
        raise TimeoutError("Fixture process descendants did not finish")

    def close(self):
        if self.job:
            # Confirm all descendants have exited before uninstallation or
            # the next singleton launch. Never terminate an unowned process.
            try:
                self.api.check(self.api.kernel.TerminateJobObject(self.job, 1), "stop private fixture job")
                self.wait_job_idle(10)
            finally:
                self.api.kernel.CloseHandle(self.job)
                self.job = None
        if self.handle:
            self.api.kernel.WaitForSingleObject(self.handle, 5000)
            self.api.kernel.CloseHandle(self.handle)
            self.handle = None
        for handle, _ in self.children.values():
            self.api.kernel.CloseHandle(handle)
        self.children.clear()


def environment(case: Path, home: Path, temporary: Path) -> dict:
    value = {k: v for k, v in os.environ.items() if not k.upper().startswith("DSH_")}
    for directory in (case, home, temporary, case / "profile", case / "local", case / "roaming"):
        directory.mkdir(parents=True, exist_ok=True)
    value.update(DSH_HOME=str(home), DSH_DESKTOP_PREFERENCES=str(case / "preferences.json"),
        DSH_DESKTOP_DIAGNOSTICS=str(case / "desktop.jsonl"),
        DSH_TELEMETRY_DISABLED="1", TEMP=str(temporary), TMP=str(temporary),
        USERPROFILE=str(case / "profile"), HOME=str(case / "profile"),
        LOCALAPPDATA=str(case / "local"), APPDATA=str(case / "roaming"))
    return value


def existing_installation():
    import winreg
    for hive in (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE):
        for view in (winreg.KEY_WOW64_32KEY, winreg.KEY_WOW64_64KEY):
            try:
                with winreg.OpenKey(hive, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\" + APP_ID,
                                   0, winreg.KEY_READ | view):
                    return True
            except FileNotFoundError:
                pass
    return False


def package_inventory(archive: Path) -> dict:
    with zipfile.ZipFile(archive) as source:
        candidates = [n for n in source.namelist() if n.endswith("/DESKTOP.json")]
        if len(candidates) != 1:
            raise ValueError("Expected one production DESKTOP.json in the portable archive")
        manifest = json.loads(source.read(candidates[0]))
    if manifest.get("platform") != "windows" or manifest.get("hostRoot") != "host" or manifest.get("entry") != "dsh_desktop.exe":
        raise ValueError("A production Windows Flutter archive is required")
    return manifest


def launch_gui(api, installation, case, home, expected, timeout, legacy=False, long_temp=False):
    temporary = case / "temp"
    if long_temp:
        while len(str(temporary).encode("utf-16-le")) // 2 < 205:
            remaining = 205 - len(str(temporary).encode("utf-16-le")) // 2
            if remaining < 2:
                temporary /= "长"
                break
            temporary /= "ready publication 路径"[:min(20, remaining - 1)]
    env = environment(case, home, temporary)
    preferences = case / "preferences.json"
    if legacy:
        preferences.write_text(json.dumps({"address": "http://127.0.0.1:58080",
            "executable": str(installation / "host/deepseek-harness-rs.exe"),
            "dark": True, "drafts": {}, "layout": {}}), encoding="utf-8")
    gui = Process(api, installation / "dsh_desktop.exe", [], installation, env)
    report = {"gui": {"pid": gui.pid, **gui.identity}, "temporaryPathLength": len(str(temporary)),
              "preferences": str(preferences), "home": str(home), "observedReadyParents": []}
    started = time.monotonic()
    try:
        while time.monotonic() - started < timeout:
            gui.track_hosts(installation / "host/deepseek-harness-rs.exe")
            for ready_parent in temporary.glob("dsh-host-ready-*"):
                record = {"path": str(ready_parent), "utf16Length": len(str(ready_parent).encode("utf-16-le")) // 2}
                if record not in report["observedReadyParents"]:
                    report["observedReadyParents"].append(record)
            if not gui.alive():
                raise RuntimeError(f"Installed GUI exited before bootstrap: {gui.wait(0)}")
            if preferences.exists():
                try:
                    saved = read_json(preferences)
                except (OSError, ValueError):
                    time.sleep(.1)
                    continue
                owner = saved.get("ownedHost")
                if owner:
                    if not isinstance(owner.get("pid"), int) or not isinstance(owner.get("instanceId"), str):
                        raise ValueError("Invalid new ownedHost record")
                    uuid.UUID(owner["instanceId"])
                    if not same_path(owner["executable"], installation / "host/deepseek-harness-rs.exe") or not same_path(owner["home"], home):
                        raise ValueError("Owned Host record selected another executable/home")
                    identity = gui.owned_host(owner["pid"], installation / "host/deepseek-harness-rs.exe")
                    described = rpc(owner["address"])
                    if described.get("processId") != owner["pid"] or described.get("instanceId") != owner["instanceId"]:
                        raise ValueError("Host RPC PID/nonce differs from the new preference record")
                    if not same_path(described["home"], home) or not same_path(described["cwd"], installation / "host"):
                        raise ValueError("Host RPC data root/cwd differs from installed bootstrap")
                    if described.get("version") != expected["version"]:
                        raise ValueError("Installed Host reports another version")
                    windows = api.windows(gui.pid)
                    if not windows:
                        time.sleep(.1)
                        continue
                    diagnostics = connected_diagnostics(case / "desktop.jsonl", gui.pid)
                    if diagnostics is None:
                        time.sleep(.1)
                        continue
                    log = Path(owner["logFile"])
                    if not log.is_relative_to(case) or "dsh: desktop Host starting" not in log_tail(log):
                        raise ValueError("Owned Host log is absent, outside the case, or lacks its startup marker")
                    report.update(ownedHost=owner, host=identity, description=described,
                        visibleGuiWindows=windows, logFile=str(log),
                        guiDiagnostics=diagnostics,
                        hostSha256=digest(Path(identity["executable"])), bootstrapVerified=True)
                    if report["hostSha256"] != expected["files"]["host/deepseek-harness-rs.exe"]["sha256"]:
                        raise ValueError("Running Host image differs from production archive")
                    time.sleep(1)
                    if not gui.alive() or rpc(owner["address"]).get("instanceId") != owner["instanceId"]:
                        raise ValueError("GUI/Host did not remain alive after bootstrap")
                    gui.owned_host(owner["pid"], installation / "host/deepseek-harness-rs.exe")
                    report["aliveAfterBootstrap"] = True
                    return gui, report
            time.sleep(.1)
        raise TimeoutError("Installed GUI did not publish a new ownedHost record")
    except BaseException as error:
        report["error"] = str(error)
        report["observedHosts"] = gui.child_reports()
        report["seconds"] = round(time.monotonic() - started, 3)
        (case / "bootstrap.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        gui.close()
        raise


def connected_diagnostics(path, pid):
    if not path.is_file():
        return None
    if path.stat().st_size > 2 * 1024 * 1024:
        raise ValueError("GUI diagnostics exceeded size bound")
    for line in reversed(path.read_text(encoding="utf-8", errors="replace").splitlines()):
        try:
            sample = json.loads(line)
        except ValueError:
            continue
        if (sample.get("pid") == pid and sample.get("connected") is True
            and sample.get("viewPhysicalWidth", 0) > 0 and sample.get("viewPhysicalHeight", 0) > 0
            and sample.get("frameReportedSamples", 0) > 0 and sample.get("controllerSubscriptions", 0) > 0):
            return sample
    return None


def baseline_long_temp(case):
    report = read_json(case / "bootstrap.json")
    logs = list((case / "host-logs").glob("*.log"))
    tail = "\n".join(log_tail(path) for path in logs)
    too_long = any(item["utf16Length"] + 1 + len(".dsh-ready-") + 36 + len(".tmp") + 1 > 260
                   for item in report["observedReadyParents"])
    exited_one = any(item.get("exitCode") == 1 for item in report.get("observedHosts", []))
    publication_failed = re.search(r"dsh: readiness publication failed:.*\(os error (?:3|206)\)", tail)
    if not (too_long and exited_one and publication_failed and "dsh: desktop Host starting" in tail):
        raise RuntimeError("Long TEMP failure did not match the r8 readiness MAX_PATH/exit-1 defect")
    report.update(expectedBaselineFailure=True, matchedFailure="r8 readiness publication MAX_PATH, observed exit 1",
                  logFiles=[str(path) for path in logs], logTail=tail)
    return report


def verify_conflict(api, installation, root, expected, args):
    binary = args.conflict_binary.resolve()
    package = read_json(binary.parent / "PACKAGE.json")
    if package.get("variant") != "core" or package.get("platform") != "windows" or package.get("host") != binary.name:
        raise ValueError("--conflict-binary requires an intact real Windows Core package")
    build = json.loads(subprocess.check_output([str(binary), "--build-info"], timeout=15))
    if build.get("revision") != R6_REVISION or build.get("dirty") is not False:
        raise ValueError("Conflict owner must be the clean, published r6 Core")
    case, owner_case = root / "legacy-conflict", root / "r6-core-owner"
    home = case / "home"
    env = environment(owner_case, home, owner_case / "temp")
    output = owner_case / "core.log"
    core = Process(api, binary, ["web", "--port", "0"], binary.parent, env, output)
    try:
        deadline = time.monotonic() + args.timeout
        address = None
        while time.monotonic() < deadline:
            if not core.alive():
                raise RuntimeError(f"Published r6 Core exited before readiness: {core.wait(0)}")
            match = re.search(r"dsh web: (http://127\.0\.0\.1:\d+)", log_tail(output))
            if match:
                address = match[1]
                break
            time.sleep(.1)
        if address is None:
            raise TimeoutError("Published r6 Core did not become ready")
        description = rpc(address)
        if not same_path(description["home"], home):
            raise ValueError("r6 Core owner uses another home")
        # The original r6 RPC predates PID/nonces. Ownership of this negative
        # fixture comes from its retained original process handle/private job.
        try:
            gui, report = launch_gui(api, installation, case, home, expected, min(args.timeout, 15), legacy=True)
        except (RuntimeError, TimeoutError) as error:
            report = read_json(case / "bootstrap.json")
            logs = list((case / "host-logs").glob("*.log"))
            tail = "\n".join(log_tail(path) for path in logs)
            if "该数据目录正在使用，请先关闭其它 Harness 实例" not in tail:
                raise RuntimeError("Conflict did not produce the concrete shared-home lock diagnostic") from error
            if (case / "preferences.json").is_file() and read_json(case / "preferences.json").get("ownedHost"):
                raise ValueError("Conflicting GUI unexpectedly saved ownership")
            if not core.alive() or not same_path(rpc(address)["home"], home):
                raise ValueError("Verifier disturbed the original r6 Core owner")
            report.update(conflictVerified=True, logTail=tail, coreOwner={"pid": core.pid, **core.identity,
                "sha256": digest(binary), "buildInfo": build, "description": description})
        else:
            stop_gui(api, gui)
            raise RuntimeError("Legacy-preference GUI unexpectedly bootstrapped while r6 Core held the same home")
    finally:
        core.close()  # Original r6 fixture job only; no discovered-PID cleanup.
    retry_case = root / "legacy-conflict-retry"
    gui, retry = launch_gui(api, installation, retry_case, home, expected, args.timeout, legacy=True)
    try:
        report["retryAfterOwnerExit"] = retry
    finally:
        stop_gui(api, gui)
    return report


def stop_gui(api, gui):
    for window in api.windows(gui.pid):
        api.user.PostMessageW(window, 0x10, 0, 0)  # WM_CLOSE, own windows only.
    try:
        gui.wait(5)
    except TimeoutError:
        pass  # Private job closure owns cleanup, regardless of app tray-close behavior.
    finally:
        gui.close()


def run(args):
    if os.name != "nt":
        raise RuntimeError("Windows is required; no mocked installed-runtime fallback")
    api = Windows()
    root = args.workdir.resolve()
    if root.exists():
        raise ValueError("--workdir must not exist; the verifier only owns fresh fixture directories")
    root.mkdir(parents=True)
    result = {"schemaVersion": 1, "passed": False, "token": api.token(),
        "installer": str(args.installer.resolve()), "installerSha256": digest(args.installer),
        "scope": "installed GUI bootstrap and real Host identity; not full UI behavior", "cases": {}}
    installation = root / "实际安装 中文 空格"
    installed = False
    install_attempted = False
    try:
        if args.require_standard_user and not result["token"]["standardUser"]:
            raise RuntimeError("This process is elevated or has a filtered administrator token; a standard-user account is required")
        if existing_installation():
            raise RuntimeError("Existing production desktop uninstall registration; refusing to alter another installation")
        if api.user.FindWindowW("FLUTTER_RUNNER_WIN32_WINDOW", "DeepSeek Harness"):
            raise RuntimeError("Existing production Flutter window; refusing singleton interference")
        mutex = api.kernel.OpenMutexW(0x100000, False, "Local\\DeepSeekHarnessFlutterDesktop")
        if mutex:
            api.kernel.CloseHandle(mutex)
            raise RuntimeError("Existing production desktop singleton; refusing interference")
        if ctypes.get_last_error() != 2:
            raise RuntimeError("Cannot establish absence of the production desktop singleton")
        expected = package_inventory(args.archive)
        if args.baseline_r8 and expected.get("sourceRevision") != R8_REVISION:
            raise ValueError("--baseline-r8 is restricted to the published r8 source revision")
        if args.require_conflict_owner and args.conflict_binary is None:
            raise ValueError("--require-conflict-owner needs --conflict-binary")
        result["archiveSha256"] = digest(args.archive)
        result["sourceRevision"] = expected.get("sourceRevision")
        install_case = root / "installer"
        env = environment(install_case, root / "installer-home", install_case / "temp")
        install_attempted = True
        installer = Process(api, args.installer.resolve(), ["/VERYSILENT", "/SUPPRESSMSGBOXES",
            "/NORESTART", "/NOICONS", "/TASKS=", f"/DIR={installation}",
            f"/LOG={install_case / 'install.log'}"], root, env)
        try:
            code = installer.wait(180)
            installer.wait_job_idle(30)
            installed = (installation / "unins000.exe").is_file()
            if code:
                raise RuntimeError(f"Production installer failed with exit {code}; see install.log")
        finally:
            installer.close()
            installed = (installation / "unins000.exe").is_file()
        for name in ("dsh_desktop.exe", "data/app.so", "flutter_windows.dll", "host/deepseek-harness-rs.exe", "host/PACKAGE.json"):
            if digest(installation / name) != expected["files"][name]["sha256"]:
                raise ValueError(f"Installed production payload differs: {name}")
        result["installedPayloadVerified"] = True
        result["guiSha256"] = digest(installation / "dsh_desktop.exe")
        for name, legacy, long_temp in (("fresh", False, False), ("legacy-r6", True, False), ("long-temp", False, True)):
            case = root / name
            try:
                gui, report = launch_gui(api, installation, case, case / "home", expected, args.timeout, legacy, long_temp)
            except (RuntimeError, TimeoutError):
                if not (long_temp and args.baseline_r8):
                    raise
                result["cases"][name] = baseline_long_temp(case)
                continue
            try:
                result["cases"][name] = report
                report["legacyPreferences"] = legacy
            finally:
                stop_gui(api, gui)
        if args.conflict_binary:
            result["cases"]["legacy-core-conflict-and-retry"] = verify_conflict(api, installation, root, expected, args)
        else:
            result["cases"]["legacy-core-conflict-and-retry"] = {"skipped": "No real r6 --conflict-binary supplied"}
        # Read/execute-only ACL applies to this installation alone. A write
        # probe must fail; an elevated verifier cannot claim this coverage.
        if result["token"]["standardUser"]:
            try:
                acl = subprocess.run(["icacls", str(installation), "/deny", "*S-1-5-32-545:(OI)(CI)(WD,AD,WEA,WA)", "/T", "/Q"], capture_output=True, timeout=60)
                if acl.returncode:
                    raise RuntimeError("Could not set fixture installation read-only ACL")
                probe = installation / ("write-probe-" + str(uuid.uuid4()))
                try:
                    probe.write_text("probe", encoding="utf-8")
                except PermissionError:
                    pass
                else:
                    probe.unlink()
                    raise RuntimeError("Installation write probe succeeded; read-only case is invalid")
                case = root / "read-only-installation"
                gui, report = launch_gui(api, installation, case, case / "home", expected, args.timeout)
                try:
                    report["installationWriteDenied"] = True
                    result["cases"]["read-only-installation"] = report
                finally:
                    stop_gui(api, gui)
            finally:
                undo = subprocess.run(["icacls", str(installation), "/remove:d", "*S-1-5-32-545", "/T", "/Q"], capture_output=True, timeout=60)
                if undo.returncode:
                    raise RuntimeError("Could not restore owned fixture installation ACL")
        else:
            result["cases"]["read-only-installation"] = {"skipped": "Requires a verified standard-user account"}
        result["passed"] = True
    except BaseException as error:
        result["error"] = str(error)
    finally:
        installed = installed or (install_attempted and (installation / "unins000.exe").is_file())
        if installed:
            try:
                uninstaller = Process(api, installation / "unins000.exe", ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"], root,
                    environment(root / "uninstall", root / "installer-home", root / "uninstall/temp"))
                try:
                    if uninstaller.wait(120):
                        raise RuntimeError("Fixture uninstaller failed")
                    uninstaller.wait_job_idle(30)
                finally:
                    uninstaller.close()
                if existing_installation():
                    raise RuntimeError("Fixture uninstall registration was not removed")
                result["fixtureUninstalled"] = True
            except BaseException as error:
                result["cleanupError"] = str(error)
                result["passed"] = False
        elif install_attempted and existing_installation():
            result["cleanupError"] = "Installer created a registration but no owned scratch uninstaller; manual fixture recovery required"
            result["passed"] = False
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installer", required=True, type=Path, help="Unmodified production Flutter setup EXE")
    parser.add_argument("--archive", required=True, type=Path, help="Matching production Flutter portable ZIP, inventory only")
    parser.add_argument("--workdir", required=True, type=Path, help="Fresh absolute fixture/log directory")
    parser.add_argument("--timeout", type=float, default=45)
    parser.add_argument("--require-standard-user", action="store_true")
    parser.add_argument("--conflict-binary", type=Path, help="Published clean r6 Host from an intact real Core package")
    parser.add_argument("--require-conflict-owner", action="store_true")
    parser.add_argument("--baseline-r8", action="store_true", help="Accept only the recorded r8 long-TEMP readiness publication/exit-1 defect")
    args = parser.parse_args()
    raise SystemExit(run(args))


if __name__ == "__main__":
    main()
