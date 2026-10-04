param([string]$RunnerTemp = $env:RUNNER_TEMP)
$ErrorActionPreference = 'Stop'

# The sole writable object is a new GUID directory beneath RUNNER_TEMP.
# Its existing host ACEs are preserved; no accounts or outside ACLs are changed.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;

public static class WindowsAclProbe
{
    const uint Full = 0x1F01FF, ReadExecute = 0x1200A9, Navigation = 0xA0;
    const uint TokenAccess = 0x01AB;
    const byte Inherited = 0x10;

    [StructLayout(LayoutKind.Sequential)]
    struct SidAttributes { public IntPtr Sid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    struct GenericMapping { public uint Read, Write, Execute, All; }
    [StructLayout(LayoutKind.Sequential)]
    struct UnicodeString { public ushort Length, MaximumLength; public IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)]
    struct ObjectAttributes
    {
        public uint Length;
        public IntPtr RootDirectory, ObjectName;
        public uint Attributes;
        public IntPtr SecurityDescriptor, SecurityQualityOfService;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct IoStatusBlock { public IntPtr Status; public UIntPtr Information; }
    [StructLayout(LayoutKind.Sequential)]
    struct Luid { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)]
    struct FileInformation
    {
        public uint Attributes, CreationLow, CreationHigh, AccessLow, AccessHigh, WriteLow, WriteHigh;
        public uint VolumeSerial, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
    }

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr pointer);
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool ConvertStringSidToSidW(string value, out IntPtr sid);
    [DllImport("advapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool IsValidSid(IntPtr sid);
    [DllImport("advapi32.dll")]
    static extern uint GetLengthSid(IntPtr sid);
    [DllImport("advapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool EqualSid(IntPtr first, IntPtr second);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CreateRestrictedToken(IntPtr existing, uint flags, uint disabledCount,
        IntPtr disabled, uint deletedCount, IntPtr deleted, uint count, IntPtr sids, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool DuplicateTokenEx(IntPtr existing, uint access, IntPtr attributes,
        int impersonationLevel, int type, out IntPtr duplicate);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool ImpersonateLoggedOnUser(IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool RevertToSelf();
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetTokenInformation(IntPtr token, int informationClass,
        IntPtr buffer, uint length, out uint returned);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool LookupPrivilegeValueW(string system, string name, out Luid luid);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    static extern uint GetNamedSecurityInfoW(string path, int type, uint information,
        out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    static extern uint SetNamedSecurityInfoW(string path, int type, uint information,
        IntPtr owner, IntPtr group, IntPtr dacl, IntPtr sacl);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool InitializeAcl(IntPtr acl, uint size, uint revision);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool AddAccessDeniedAceEx(IntPtr acl, uint revision, uint flags, uint mask, IntPtr sid);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool AddAccessAllowedAceEx(IntPtr acl, uint revision, uint flags, uint mask, IntPtr sid);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool AddAce(IntPtr acl, uint revision, uint index, IntPtr ace, uint size);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetAce(IntPtr acl, uint index, out IntPtr ace);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool AccessCheck(IntPtr descriptor, IntPtr impersonationToken, uint desired,
        ref GenericMapping mapping, IntPtr privilegeSet, ref uint length, out uint granted,
        [MarshalAs(UnmanagedType.Bool)] out bool accessStatus);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    static extern IntPtr CreateFileW(string path, uint access, uint share, IntPtr security,
        uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetFileInformationByHandle(IntPtr handle, out FileInformation information);
    [DllImport("ntdll.dll", ExactSpelling = true)]
    static extern int NtOpenFile(out IntPtr handle, uint desired, ref ObjectAttributes attributes,
        out IoStatusBlock status, uint share, uint options);
    [DllImport("ntdll.dll", ExactSpelling = true)]
    static extern uint RtlNtStatusToDosError(int status);

    public class TokenInfo
    {
        public string Name { get; set; }
        public int Type { get; set; }
        public int RestrictedSidCount { get; set; }
        public bool ChangeNotifyPresent { get; set; }
        public bool ChangeNotifyEnabled { get; set; }
        public uint ChangeNotifyAttributes { get; set; }
    }
    public class AceInfo
    {
        public string Principal { get; set; }
        public byte Type { get; set; }
        public byte Flags { get; set; }
        public uint Mask { get; set; }
    }
    public class Result
    {
        public string Token { get; set; }
        public string Api { get; set; }
        public uint Desired { get; set; }
        public bool ApiSuccess { get; set; }
        public bool Allowed { get; set; }
        public uint Granted { get; set; }
        public uint Error { get; set; }
        public string NtStatus { get; set; }
        public bool? MetadataQuerySucceeded { get; set; }
        public uint MetadataError { get; set; }
        public bool? IsDirectory { get; set; }
    }
    public class Report
    {
        public string Directory { get; set; }
        public int PointerBytes { get; set; }
        public int UnicodeStringBytes { get; set; }
        public int ObjectAttributesBytes { get; set; }
        public int IoStatusBlockBytes { get; set; }
        public int HostAceCountBefore { get; set; }
        public int HostAceCountAfter { get; set; }
        public List<TokenInfo> Tokens { get; set; } = new List<TokenInfo>();
        public List<AceInfo> ManagedAces { get; set; } = new List<AceInfo>();
        public List<Result> Results { get; set; } = new List<Result>();
        public bool Completed { get; set; }
        public bool ExactNavigationPassed { get; set; }
        public bool CleanupSucceeded { get; set; }
        public string FatalError { get; set; }
        public string CleanupError { get; set; }
    }

    static void Check(bool ok, string operation)
    {
        int error = Marshal.GetLastWin32Error();
        if (!ok) throw new InvalidOperationException(operation + " failed: " + error);
    }
    static IntPtr Sid(string value)
    {
        IntPtr sid;
        Check(ConvertStringSidToSidW(value, out sid), "ConvertStringSidToSidW");
        if (!IsValidSid(sid)) { LocalFree(sid); throw new InvalidOperationException("Invalid synthetic SID"); }
        return sid;
    }
    static IntPtr Information(IntPtr token, int informationClass)
    {
        uint length;
        GetTokenInformation(token, informationClass, IntPtr.Zero, 0, out length);
        if (length < 4 || length > 1048576) throw new InvalidOperationException("Invalid token info size");
        IntPtr data = Marshal.AllocHGlobal((int)length);
        try
        {
            uint returned;
            Check(GetTokenInformation(token, informationClass, data, length, out returned), "GetTokenInformation");
            return data;
        }
        catch { Marshal.FreeHGlobal(data); throw; }
    }
    static TokenInfo Inspect(IntPtr token, string name)
    {
        var info = new TokenInfo { Name = name };
        IntPtr type = Information(token, 8), groups = IntPtr.Zero, privileges = IntPtr.Zero;
        try
        {
            info.Type = Marshal.ReadInt32(type);
            groups = Information(token, 11);
            info.RestrictedSidCount = Marshal.ReadInt32(groups);
            Luid notify;
            Check(LookupPrivilegeValueW(null, "SeChangeNotifyPrivilege", out notify), "LookupPrivilegeValueW");
            privileges = Information(token, 3);
            int count = Marshal.ReadInt32(privileges);
            for (int i = 0; i < count; i++)
            {
                IntPtr entry = IntPtr.Add(privileges, 4 + 12 * i);
                if ((uint)Marshal.ReadInt32(entry) == notify.Low && Marshal.ReadInt32(entry, 4) == notify.High)
                {
                    info.ChangeNotifyPresent = true;
                    info.ChangeNotifyAttributes = (uint)Marshal.ReadInt32(entry, 8);
                    info.ChangeNotifyEnabled = (info.ChangeNotifyAttributes & 2) != 0;
                }
            }
        }
        finally
        {
            Marshal.FreeHGlobal(type);
            if (groups != IntPtr.Zero) Marshal.FreeHGlobal(groups);
            if (privileges != IntPtr.Zero) Marshal.FreeHGlobal(privileges);
        }
        return info;
    }
    static IntPtr Restricted(IntPtr existing, params IntPtr[] sids)
    {
        int size = Marshal.SizeOf(typeof(SidAttributes));
        IntPtr array = Marshal.AllocHGlobal(size * sids.Length);
        try
        {
            Marshal.Copy(new byte[size * sids.Length], 0, array, size * sids.Length);
            for (int i = 0; i < sids.Length; i++)
                Marshal.StructureToPtr(new SidAttributes { Sid = sids[i], Attributes = 0 }, IntPtr.Add(array, size * i), false);
            IntPtr token;
            Check(CreateRestrictedToken(existing, 1, 0, IntPtr.Zero, 0, IntPtr.Zero,
                (uint)sids.Length, array, out token), "CreateRestrictedToken FLAGS=1");
            return token;
        }
        finally { Marshal.FreeHGlobal(array); }
    }
    static IntPtr Descriptor(string path, out IntPtr dacl)
    {
        IntPtr owner, group, sacl, descriptor;
        uint status = GetNamedSecurityInfoW(path, 1, 7, out owner, out group, out dacl, out sacl, out descriptor);
        if (status != 0) throw new InvalidOperationException("GetNamedSecurityInfoW failed: " + status);
        return descriptor;
    }
    static void Apply(string path, IntPtr account, IntPtr group, Report report)
    {
        IntPtr originalDacl;
        IntPtr original = Descriptor(path, out originalDacl), replacement = IntPtr.Zero;
        try
        {
            int count = (ushort)Marshal.ReadInt16(originalDacl, 4);
            report.HostAceCountBefore = count;
            var kept = new List<IntPtr>();
            int bytes = 8 + 3 * (8 + (int)GetLengthSid(account)) + 2 * (8 + (int)GetLengthSid(group));
            for (int i = 0; i < count; i++)
            {
                IntPtr ace;
                Check(GetAce(originalDacl, (uint)i, out ace), "GetAce original");
                kept.Add(ace);
                bytes += (ushort)Marshal.ReadInt16(ace, 2);
            }
            if (bytes > 65535) throw new InvalidOperationException("Probe ACL too large");
            replacement = Marshal.AllocHGlobal(bytes);
            uint revision = Marshal.ReadByte(originalDacl);
            Check(InitializeAcl(replacement, (uint)bytes, revision), "InitializeAcl");
            Check(AddAccessDeniedAceEx(replacement, revision, 0, Full & ~Navigation, account), "account self DENY");
            Check(AddAccessDeniedAceEx(replacement, revision, 0x0B, Full, account), "account inherit-only DENY");
            Check(AddAccessDeniedAceEx(replacement, revision, 0, ReadExecute & ~Navigation, group), "group self DENY");
            Check(AddAccessDeniedAceEx(replacement, revision, 0x0B, ReadExecute, group), "group inherit-only DENY");
            foreach (IntPtr ace in kept)
                if ((Marshal.ReadByte(ace, 1) & Inherited) == 0)
                    Check(AddAce(replacement, revision, UInt32.MaxValue, ace, (ushort)Marshal.ReadInt16(ace, 2)), "preserve explicit host ACE");
            Check(AddAccessAllowedAceEx(replacement, revision, 0, Navigation, account), "account navigation ALLOW");
            foreach (IntPtr ace in kept)
                if ((Marshal.ReadByte(ace, 1) & Inherited) != 0)
                    Check(AddAce(replacement, revision, UInt32.MaxValue, ace, (ushort)Marshal.ReadInt16(ace, 2)), "preserve inherited host ACE");
            uint status = SetNamedSecurityInfoW(path, 1, 0x80000004, IntPtr.Zero, IntPtr.Zero, replacement, IntPtr.Zero);
            if (status != 0) throw new InvalidOperationException("SetNamedSecurityInfoW failed: " + status);
        }
        finally { if (replacement != IntPtr.Zero) Marshal.FreeHGlobal(replacement); LocalFree(original); }
    }
    static void Readback(IntPtr dacl, IntPtr account, IntPtr group, Report report)
    {
        int count = (ushort)Marshal.ReadInt16(dacl, 4);
        for (int i = 0; i < count; i++)
        {
            IntPtr ace;
            Check(GetAce(dacl, (uint)i, out ace), "GetAce readback");
            byte type = Marshal.ReadByte(ace);
            if (type != 0 && type != 1) { report.HostAceCountAfter++; continue; }
            IntPtr sid = IntPtr.Add(ace, 8);
            string principal = EqualSid(sid, account) ? "accountA" : EqualSid(sid, group) ? "group" : null;
            if (principal == null) report.HostAceCountAfter++;
            else report.ManagedAces.Add(new AceInfo {
                Principal = principal, Type = type, Flags = Marshal.ReadByte(ace, 1), Mask = (uint)Marshal.ReadInt32(ace, 4)
            });
        }
    }
    static Result CheckAccess(IntPtr descriptor, IntPtr token, string name, uint desired)
    {
        var mapping = new GenericMapping { Read = 0x120089, Write = 0x120116, Execute = 0x1200A0, All = Full };
        uint length = 1024, granted;
        bool access;
        IntPtr privileges = Marshal.AllocHGlobal((int)length);
        try
        {
            bool ok = AccessCheck(descriptor, token, desired, ref mapping, privileges, ref length, out granted, out access);
            int error = Marshal.GetLastWin32Error();
            if (!ok && error == 122 && length <= 65536)
            {
                Marshal.FreeHGlobal(privileges);
                privileges = IntPtr.Zero;
                privileges = Marshal.AllocHGlobal((int)length);
                ok = AccessCheck(descriptor, token, desired, ref mapping, privileges, ref length, out granted, out access);
                error = Marshal.GetLastWin32Error();
            }
            return new Result { Token = name, Api = "AccessCheck", Desired = desired,
                ApiSuccess = ok, Allowed = ok && access, Granted = granted, Error = ok ? 0 : (uint)error };
        }
        finally { if (privileges != IntPtr.Zero) Marshal.FreeHGlobal(privileges); }
    }
    static Result Open(string path, string name, uint desired, bool native, bool overlapped)
    {
        IntPtr handle = IntPtr.Zero, text = IntPtr.Zero, unicode = IntPtr.Zero;
        try
        {
            if (!native)
            {
                handle = CreateFileW(path, desired, 7, IntPtr.Zero, 3, 0x02000000U | (overlapped ? 0x40000000U : 0), IntPtr.Zero);
                int error = Marshal.GetLastWin32Error();
                bool ok = handle != new IntPtr(-1);
                return new Result { Token = name, Api = overlapped ? "CreateFileW_overlapped" : "CreateFileW_plain",
                    Desired = desired, ApiSuccess = ok, Allowed = ok, Error = ok ? 0 : (uint)error };
            }
            string ntPath = @"\??\" + path;
            int bytes = checked(ntPath.Length * 2);
            if (bytes > 65532) throw new InvalidOperationException("NT probe path too long");
            text = Marshal.StringToHGlobalUni(ntPath);
            var value = new UnicodeString { Length = (ushort)bytes, MaximumLength = (ushort)(bytes + 2), Buffer = text };
            unicode = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(UnicodeString)));
            Marshal.StructureToPtr(value, unicode, false);
            var attributes = new ObjectAttributes {
                Length = (uint)Marshal.SizeOf(typeof(ObjectAttributes)), ObjectName = unicode, Attributes = 0x40
            };
            IoStatusBlock io;
            int status = NtOpenFile(out handle, desired, ref attributes, out io, 7, 1); // DIRECTORY only, no synchronous I/O options.
            bool success = status == 0 && handle != IntPtr.Zero && handle != new IntPtr(-1);
            var result = new Result { Token = name, Api = "NtOpenFile_exact", Desired = desired, ApiSuccess = success,
                Allowed = success, Error = status == 0 ? 0 : RtlNtStatusToDosError(status), NtStatus = "0x" + ((uint)status).ToString("X8") };
            if (success && desired == Navigation)
            {
                FileInformation information;
                bool queried = GetFileInformationByHandle(handle, out information);
                int error = Marshal.GetLastWin32Error();
                result.MetadataQuerySucceeded = queried;
                result.MetadataError = queried ? 0 : (uint)error;
                result.IsDirectory = queried ? (bool?)((information.Attributes & 0x10) != 0) : null;
            }
            return result;
        }
        finally
        {
            if (handle != IntPtr.Zero && handle != new IntPtr(-1)) CloseHandle(handle);
            if (unicode != IntPtr.Zero) Marshal.FreeHGlobal(unicode);
            if (text != IntPtr.Zero) Marshal.FreeHGlobal(text);
        }
    }
    static void Matrix(string path, IntPtr descriptor, IntPtr token, string name, Report report)
    {
        IntPtr impersonation;
        Check(DuplicateTokenEx(token, 0x0C, IntPtr.Zero, 2, 2, out impersonation), "DuplicateTokenEx for AccessCheck");
        uint[] masks = { 0x80, 0x20, 0xA0, 0x01, 0x100000, 0x1000A0 };
        try
        {
            foreach (uint desired in masks) report.Results.Add(CheckAccess(descriptor, impersonation, name, desired));
        }
        finally { CloseHandle(impersonation); }
        Check(ImpersonateLoggedOnUser(token), "ImpersonateLoggedOnUser");
        try
        {
            foreach (uint desired in masks)
            {
                report.Results.Add(Open(path, name, desired, true, false));
                report.Results.Add(Open(path, name, desired, false, true));
                report.Results.Add(Open(path, name, desired, false, false));
            }
        }
        finally { Check(RevertToSelf(), "RevertToSelf"); }
    }
    public static Report Run(string runnerTemp)
    {
        var report = new Report {
            PointerBytes = IntPtr.Size, UnicodeStringBytes = Marshal.SizeOf(typeof(UnicodeString)),
            ObjectAttributesBytes = Marshal.SizeOf(typeof(ObjectAttributes)), IoStatusBlockBytes = Marshal.SizeOf(typeof(IoStatusBlock))
        };
        IntPtr host = IntPtr.Zero, accountToken = IntPtr.Zero, sdkToken = IntPtr.Zero;
        IntPtr account = IntPtr.Zero, group = IntPtr.Zero, cap = IntPtr.Zero, descriptor = IntPtr.Zero;
        bool created = false;
        try
        {
            if (String.IsNullOrWhiteSpace(runnerTemp) || !Directory.Exists(runnerTemp))
                throw new InvalidOperationException("RUNNER_TEMP must name an existing runner directory");
            report.Directory = Path.Combine(Path.GetFullPath(runnerTemp), "dsh-acl-probe-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(report.Directory);
            created = true;
            Check(OpenProcessToken(GetCurrentProcess(), TokenAccess, out host), "OpenProcessToken");
            account = Sid("S-1-5-21-678-901-234-1001");
            group = Sid("S-1-5-21-678-901-234-2001");
            cap = Sid("S-1-5-21-432-765-987-3001");
            accountToken = Restricted(host, account, group);
            sdkToken = Restricted(host, account, group, cap);
            report.Tokens.Add(Inspect(host, "host"));
            report.Tokens.Add(Inspect(accountToken, "account_group"));
            report.Tokens.Add(Inspect(sdkToken, "account_group_cap"));
            Apply(report.Directory, account, group, report);
            IntPtr dacl;
            descriptor = Descriptor(report.Directory, out dacl);
            Readback(dacl, account, group, report);
            Matrix(report.Directory, descriptor, host, "host", report);
            Matrix(report.Directory, descriptor, accountToken, "account_group", report);
            Matrix(report.Directory, descriptor, sdkToken, "account_group_cap", report);
            bool expected = true;
            int checkedCount = 0;
            foreach (var result in report.Results)
            {
                if (result.Token == "host" || (result.Api != "AccessCheck" && result.Api != "NtOpenFile_exact")) continue;
                bool allow = result.Desired == 0x80 || result.Desired == 0x20 || result.Desired == 0xA0;
                expected &= result.Allowed == allow && (result.Api != "AccessCheck" || result.ApiSuccess);
                if (!allow && result.Api == "NtOpenFile_exact") expected &= result.Error == 5 && result.NtStatus == "0xC0000022";
                if (allow && result.Desired == Navigation && result.Api == "NtOpenFile_exact")
                    expected &= result.MetadataQuerySucceeded == true && result.IsDirectory == true;
                checkedCount++;
            }
            report.ExactNavigationPassed = checkedCount == 24 && expected;
            report.Completed = true;
        }
        catch (Exception error) { report.FatalError = error.GetType().Name + ": " + error.Message; }
        finally
        {
            if (descriptor != IntPtr.Zero) LocalFree(descriptor);
            if (sdkToken != IntPtr.Zero) CloseHandle(sdkToken);
            if (accountToken != IntPtr.Zero) CloseHandle(accountToken);
            if (host != IntPtr.Zero) CloseHandle(host);
            if (cap != IntPtr.Zero) LocalFree(cap);
            if (group != IntPtr.Zero) LocalFree(group);
            if (account != IntPtr.Zero) LocalFree(account);
            if (created)
            {
                try { Directory.Delete(report.Directory, false); report.CleanupSucceeded = true; }
                catch (Exception error) { report.CleanupError = error.GetType().Name + ": " + error.Message; }
            }
        }
        return report;
    }
}
'@

$report = [WindowsAclProbe]::Run($RunnerTemp)
$report | ConvertTo-Json -Depth 8
$details = @($report.Results | Where-Object { $_.Desired -eq 0xA0 } | ForEach-Object {
    "$($_.Token)/$($_.Api)=$($_.Allowed)/Win32=$($_.Error)/NT=$($_.NtStatus)/metadata=$($_.MetadataQuerySucceeded)/dir=$($_.IsDirectory)"
}) -join '; '
$summary = "Complete=$($report.Completed); exactNavigation=$($report.ExactNavigationPassed); hostACEs=$($report.HostAceCountBefore)/$($report.HostAceCountAfter); cleanup=$($report.CleanupSucceeded); $details"
if ($report.FatalError) { $summary += "; fatal=$($report.FatalError)" }
if ($report.CleanupError) { $summary += "; cleanupError=$($report.CleanupError)" }
$summary = $summary.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
if ($summary.Length -gt 3800) { $summary = $summary.Substring(0, 3800) }
while ([Text.Encoding]::UTF8.GetByteCount($summary) -gt 3800) { $summary = $summary.Substring(0, $summary.Length - 1) }
if (!$report.Completed -or !$report.ExactNavigationPassed -or !$report.CleanupSucceeded -or $report.FatalError) {
    Write-Output "::error title=Windows ACL API probe::$summary"
    exit 1
}
Write-Output "::notice title=Windows ACL API probe::$summary"
exit 0
