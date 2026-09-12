function Invoke-DshNativeProbe {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true, Position = 0)]
        [ValidateNotNullOrEmpty()]
        [ValidateLength(1, 32767)]
        [string]$FilePath,
        [Parameter(Position = 1)]
        [AllowEmptyCollection()]
        [string[]]$ArgumentList = @()
    )
    $dshProbePreviousExit = $global:LASTEXITCODE
    $dshProbeExecutable = $FilePath
    $dshProbeExit = 127
    $dshProbeOutput = [System.Text.StringBuilder]::new()
    $dshProbeTruncated = $false
    try {
        $dshProbeApplication = Get-Command -Name ([System.Management.Automation.WildcardPattern]::Escape($FilePath)) -CommandType Application -ErrorAction Stop | Select-Object -First 1
        $dshProbeExecutable = $dshProbeApplication.Source
        # Windows PowerShell turns redirected native stderr into ErrorRecords.
        # Scope these preferences to the probe so it can return the complete
        # diagnostic without changing the caller's fail-fast command policy.
        $ErrorActionPreference = 'Continue'
        $PSNativeCommandUseErrorActionPreference = $false
        & $dshProbeExecutable @ArgumentList 2>&1 | ForEach-Object {
            $dshProbeLine = [string]$_
            $dshProbeRoom = 65536 - $dshProbeOutput.Length
            if ($dshProbeRoom -gt 0) {
                [void]$dshProbeOutput.Append($dshProbeLine.Substring(0, [Math]::Min($dshProbeLine.Length, $dshProbeRoom)))
                if ($dshProbeOutput.Length -lt 65536) {
                    $dshProbeNewline = [Environment]::NewLine
                    [void]$dshProbeOutput.Append($dshProbeNewline.Substring(0, [Math]::Min($dshProbeNewline.Length, 65536 - $dshProbeOutput.Length)))
                }
            }
            if ($dshProbeLine.Length -gt $dshProbeRoom) { $dshProbeTruncated = $true }
        }
        $dshProbeExit = $LASTEXITCODE
    }
    catch {
        [void]$dshProbeOutput.Clear()
        $dshProbeFailure = [string]$_
        [void]$dshProbeOutput.Append($dshProbeFailure.Substring(0, [Math]::Min(65536, $dshProbeFailure.Length)))
        $dshProbeTruncated = $dshProbeFailure.Length -gt 65536
    }
    finally {
        $global:LASTEXITCODE = $dshProbePreviousExit
    }
    [pscustomobject]@{
        Executable = $dshProbeExecutable
        ExitCode = $dshProbeExit
        Output = $dshProbeOutput.ToString()
        Truncated = $dshProbeTruncated
    }
}
