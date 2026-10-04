$ErrorActionPreference = 'Stop'

# In-memory Win32 API diagnostics only: no accounts, ACLs, or filesystem probes.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class WindowsTokenProbe
{
    const uint FixtureAccess = 0x01AB; // Exact token.rs QUERY/DUPLICATE/ASSIGN/ADJUST rights.

    [StructLayout(LayoutKind.Sequential)]
    struct SidAndAttributes
    {
        public IntPtr Sid;
        public uint Attributes;
    }

    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr memory);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool ConvertStringSidToSidW(string sid, out IntPtr pointer);
    [DllImport("advapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool IsValidSid(IntPtr sid);
    [DllImport("advapi32.dll")]
    static extern uint GetLengthSid(IntPtr sid);
    [DllImport("advapi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool IsTokenRestricted(IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetTokenInformation(IntPtr token, int informationClass,
        IntPtr buffer, uint length, out uint returned);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool DuplicateTokenEx(IntPtr existing, uint access,
        IntPtr attributes, int impersonationLevel, int tokenType, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CreateRestrictedToken(IntPtr existing, uint flags,
        uint disableCount, IntPtr disabled, uint deleteCount, IntPtr deleted,
        uint restrictedCount, IntPtr restrictions, out IntPtr token);

    public class TokenInfo
    {
        public int? Type { get; set; }
        public int TypeError { get; set; }
        public bool IsRestricted { get; set; }
        public int? RestrictedSidCount { get; set; }
        public int RestrictedSidsError { get; set; }
    }

    public class SidInfo
    {
        public string Name { get; set; }
        public string Sid { get; set; }
        public bool Valid { get; set; }
        public uint Length { get; set; }
        public int ConvertError { get; set; }
    }

    public class CaseInfo
    {
        public string Input { get; set; }
        public string Name { get; set; }
        public uint Flags { get; set; }
        public uint DisableCount { get; set; }
        public uint DeleteCount { get; set; }
        public uint RestrictCount { get; set; }
        public uint RestrictAttributes { get; set; }
        public bool ExactFixture { get; set; }
        public bool Success { get; set; }
        public int Error { get; set; }
        public TokenInfo Output { get; set; }
    }

    public class DuplicateInfo
    {
        public int RequestedType { get; set; }
        public bool Success { get; set; }
        public int Error { get; set; }
        public TokenInfo Output { get; set; }
    }

    public class Report
    {
        public string OsVersion { get; set; }
        public int PointerBytes { get; set; }
        public int SidAndAttributesBytes { get; set; }
        public int AttributesOffset { get; set; }
        public uint BaseDesiredAccess { get; set; }
        public int OpenTokenError { get; set; }
        public TokenInfo Base { get; set; }
        public List<SidInfo> Sids { get; set; } = new List<SidInfo>();
        public List<CaseInfo> Cases { get; set; } = new List<CaseInfo>();
        public List<DuplicateInfo> Duplicates { get; set; } = new List<DuplicateInfo>();
        public bool ExactFixtureCasesPassed { get; set; }
        public string FatalError { get; set; }
    }

    static TokenInfo Inspect(IntPtr token)
    {
        var info = new TokenInfo { IsRestricted = IsTokenRestricted(token) };
        IntPtr type = Marshal.AllocHGlobal(4);
        try
        {
            uint returned;
            bool ok = GetTokenInformation(token, 8, type, 4, out returned); // TokenType.
            int error = Marshal.GetLastWin32Error();
            if (ok && returned >= 4) info.Type = Marshal.ReadInt32(type);
            else info.TypeError = ok ? 87 : error;
        }
        finally { Marshal.FreeHGlobal(type); }

        uint needed;
        bool sized = GetTokenInformation(token, 11, IntPtr.Zero, 0, out needed); // TokenRestrictedSids.
        int sizeError = Marshal.GetLastWin32Error();
        if ((!sized && sizeError != 122) || needed < 4 || needed > 1048576)
        {
            info.RestrictedSidsError = !sized ? sizeError : 87;
            return info;
        }
        IntPtr groups = Marshal.AllocHGlobal((int)needed);
        try
        {
            uint returned;
            bool ok = GetTokenInformation(token, 11, groups, needed, out returned);
            int error = Marshal.GetLastWin32Error();
            if (ok && returned >= 4) info.RestrictedSidCount = Marshal.ReadInt32(groups);
            else info.RestrictedSidsError = ok ? 87 : error;
        }
        finally { Marshal.FreeHGlobal(groups); }
        return info;
    }

    static void Case(Report report, IntPtr existing, string input, string name,
        uint flags, IntPtr[] sids, bool exact)
    {
        IntPtr array = IntPtr.Zero;
        IntPtr output = IntPtr.Zero;
        try
        {
            if (sids.Length > 0)
            {
                int size = Marshal.SizeOf(typeof(SidAndAttributes));
                array = Marshal.AllocHGlobal(size * sids.Length);
                Marshal.Copy(new byte[size * sids.Length], 0, array, size * sids.Length);
                for (int i = 0; i < sids.Length; i++)
                    Marshal.StructureToPtr(new SidAndAttributes { Sid = sids[i], Attributes = 0 },
                        IntPtr.Add(array, i * size), false);
            }
            bool ok = CreateRestrictedToken(existing, flags, 0, IntPtr.Zero, 0,
                IntPtr.Zero, (uint)sids.Length, array, out output);
            int error = Marshal.GetLastWin32Error(); // Capture before any other API.
            report.Cases.Add(new CaseInfo {
                Input = input, Name = name, Flags = flags, RestrictCount = (uint)sids.Length,
                ExactFixture = exact, Success = ok, Error = ok ? 0 : error,
                Output = ok ? Inspect(output) : null
            });
        }
        finally
        {
            if (output != IntPtr.Zero) CloseHandle(output);
            if (array != IntPtr.Zero) Marshal.FreeHGlobal(array);
        }
    }

    static void Cases(Report report, IntPtr token, string input, IntPtr account,
        IntPtr group, IntPtr cap, bool exact)
    {
        var accountGroup = new[] { account, group };
        var accountGroupCap = new[] { account, group, cap };
        Case(report, token, input, "flags1_account_group", 1, accountGroup, exact);
        Case(report, token, input, "flags1_account_group_cap", 1, accountGroupCap, exact);
        // Independent comparisons, never used as a fallback or a success gate.
        Case(report, token, input, "flags5_account_group", 5, accountGroup, false);
        Case(report, token, input, "flags5_account_group_cap", 5, accountGroupCap, false);
        Case(report, token, input, "flags0_account_group", 0, accountGroup, false);
        Case(report, token, input, "flags0_account_group_cap", 0, accountGroupCap, false);
        Case(report, token, input, "flags1_no_restrictions", 1, new IntPtr[0], false);
    }

    public static Report Run()
    {
        var report = new Report {
            OsVersion = Environment.OSVersion.VersionString,
            PointerBytes = IntPtr.Size,
            SidAndAttributesBytes = Marshal.SizeOf(typeof(SidAndAttributes)),
            AttributesOffset = Marshal.OffsetOf(typeof(SidAndAttributes), "Attributes").ToInt32(),
            BaseDesiredAccess = FixtureAccess
        };
        IntPtr token = IntPtr.Zero;
        var allocations = new List<IntPtr>();
        try
        {
            bool opened = OpenProcessToken(GetCurrentProcess(), FixtureAccess, out token);
            int openError = Marshal.GetLastWin32Error();
            report.OpenTokenError = opened ? 0 : openError;
            if (!opened) throw new InvalidOperationException("OpenProcessToken failed: " + openError);
            report.Base = Inspect(token);
            string[] names = { "accountA", "group", "capA" };
            string[] strings = {
                "S-1-5-21-678-901-234-1001",
                "S-1-5-21-678-901-234-2001",
                "S-1-15-3-1024-678-901-234-1001"
            };
            for (int i = 0; i < strings.Length; i++)
            {
                IntPtr sid;
                bool converted = ConvertStringSidToSidW(strings[i], out sid);
                int error = Marshal.GetLastWin32Error();
                if (sid != IntPtr.Zero) allocations.Add(sid);
                bool valid = converted && IsValidSid(sid);
                report.Sids.Add(new SidInfo {
                    Name = names[i], Sid = strings[i], Valid = valid,
                    Length = valid ? GetLengthSid(sid) : 0, ConvertError = converted ? 0 : error
                });
                if (!valid) throw new InvalidOperationException("Invalid synthetic SID: " + names[i]);
            }
            Cases(report, token, "process", allocations[0], allocations[1], allocations[2], true);
            for (int type = 1; type <= 2; type++)
            {
                IntPtr duplicate = IntPtr.Zero;
                try
                {
                    bool ok = DuplicateTokenEx(token, FixtureAccess, IntPtr.Zero, 2, type, out duplicate);
                    int error = Marshal.GetLastWin32Error();
                    report.Duplicates.Add(new DuplicateInfo {
                        RequestedType = type, Success = ok, Error = ok ? 0 : error,
                        Output = ok ? Inspect(duplicate) : null
                    });
                    if (ok) Cases(report, duplicate, type == 1 ? "duplicate_primary" : "duplicate_impersonation",
                        allocations[0], allocations[1], allocations[2], false);
                }
                finally { if (duplicate != IntPtr.Zero) CloseHandle(duplicate); }
            }
            int exactCount = 0;
            bool exactPassed = true;
            foreach (var result in report.Cases)
                if (result.ExactFixture) { exactCount++; exactPassed &= result.Success; }
            report.ExactFixtureCasesPassed = exactCount == 2 && exactPassed;
        }
        catch (Exception error) { report.FatalError = error.GetType().Name + ": " + error.Message; }
        finally
        {
            foreach (var sid in allocations) LocalFree(sid);
            if (token != IntPtr.Zero) CloseHandle(token);
        }
        return report;
    }
}
'@

$report = [WindowsTokenProbe]::Run()
$report | ConvertTo-Json -Depth 8
$details = @($report.Cases | ForEach-Object {
    "$($_.Input)/$($_.Name)=$($_.Success)/Win32=$($_.Error)"
}) -join '; '
$summary = "Exact fixture FLAGS=1 passed=$($report.ExactFixtureCasesPassed); baseType=$($report.Base.Type); baseRestricted=$($report.Base.IsRestricted); baseRestrictSidCount=$($report.Base.RestrictedSidCount); $details"
if ($report.FatalError) { $summary += "; fatal=$($report.FatalError)" }
$summary = $summary.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
if ($summary.Length -gt 3800) { $summary = $summary.Substring(0, 3800) }
while ([Text.Encoding]::UTF8.GetByteCount($summary) -gt 3800) {
    $summary = $summary.Substring(0, $summary.Length - 1)
}
if (!$report.ExactFixtureCasesPassed -or $report.FatalError) {
    Write-Output "::error title=Windows token API probe::$summary"
    exit 1
}
Write-Output "::notice title=Windows token API probe::$summary"
exit 0
