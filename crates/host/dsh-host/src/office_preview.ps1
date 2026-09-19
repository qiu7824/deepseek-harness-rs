$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$inputPath = $env:DSH_OFFICE_INPUT
$outputPath = $env:DSH_OFFICE_OUTPUT
$extension = [IO.Path]::GetExtension($inputPath).ToLowerInvariant()
$application = $null
$document = $null
$owned = $false
try {
    $progId = switch ($extension) { '.docx' { 'KWps.Application' } '.xlsx' { 'KET.Application' } '.pptx' { 'KWPP.Application' } default { throw 'Unsupported Office format' } }
    $processName = switch ($extension) { '.docx' { 'wps' } '.xlsx' { 'et' } '.pptx' { 'wpp' } }
    $existingProcessIds = @(Get-Process -Name $processName -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    $activationStarted = Get-Date
    $application = New-Object -ComObject $progId
    $count = switch ($extension) { '.docx' { $application.Documents.Count } '.xlsx' { $application.Workbooks.Count } '.pptx' { $application.Presentations.Count } }
    if ($count -ne 0) { throw 'WPS has open documents; close them or open this document manually before retrying preview' }
    # WPS does not expose Word.Application.Hwnd consistently. Its new automation
    # document process is identified independently of pre-existing helper/UI PIDs.
    $created = @(Get-CimInstance Win32_Process -Filter ("Name='" + $processName + ".exe'") | Where-Object {
        $_.CreationDate -ge $activationStarted.AddSeconds(-1) -and
        $existingProcessIds -notcontains $_.ProcessId -and
        $_.CommandLine -match '/Automation' -and $_.CommandLine -notmatch '/prometheus'
    })
    if ($created.Count -ne 1) { throw 'WPS reused an existing application; use WPS directly or retry after closing that application' }
    $officeProcessId = $created[0].ProcessId
    $officeProcess = Get-Process -Id $officeProcessId
    $owned = $true
    $watcherPath = Join-Path ([IO.Path]::GetDirectoryName($outputPath)) 'watch-wps.ps1'
    @'
param([int]$OwnerId,[int]$OfficeId,[long]$StartedTicks)
$deadline = [DateTime]::UtcNow.AddSeconds(95)
while ($true) {
    $office = Get-Process -Id $OfficeId -ErrorAction SilentlyContinue
    if ($null -eq $office -or $office.StartTime.Ticks -ne $StartedTicks) { exit }
    if ([DateTime]::UtcNow -ge $deadline -or -not (Get-Process -Id $OwnerId -ErrorAction SilentlyContinue)) {
        Stop-Process -InputObject $office -Force -ErrorAction SilentlyContinue
        exit
    }
    Start-Sleep -Milliseconds 500
}
'@ | Set-Content -LiteralPath $watcherPath -Encoding UTF8
    $watcherArguments = @('-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',('"' + $watcherPath + '"'),'-OwnerId',$PID,'-OfficeId',$officeProcessId,'-StartedTicks',$officeProcess.StartTime.Ticks)
    Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList $watcherArguments -WindowStyle Hidden | Out-Null
    $application.AutomationSecurity = 3
    switch ($extension) {
        '.docx' {
            $application.Visible = $false
            $application.DisplayAlerts = 0
            $document = $application.Documents.Open($inputPath, $false, $true)
            $document.ExportAsFixedFormat($outputPath, 17)
        }
        '.xlsx' {
            $application.Visible = $false
            $application.DisplayAlerts = $false
            $application.AskToUpdateLinks = $false
            $document = $application.Workbooks.Open($inputPath, 0, $true)
            $document.ExportAsFixedFormat(0, $outputPath)
        }
        '.pptx' {
            $document = $application.Presentations.Open($inputPath, $true, $false, $false)
            $document.SaveAs($outputPath, 32)
        }
    }
} catch {
    $messageBytes = [Text.Encoding]::UTF8.GetBytes($_.Exception.Message + [Environment]::NewLine)
    [Console]::OpenStandardError().Write($messageBytes, 0, $messageBytes.Length)
    exit 1
} finally {
    if ($null -ne $document) {
        try { if ($extension -eq '.pptx') { $document.Close() } else { $document.Close($false) } } catch {}
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($document)
    }
    if ($null -ne $application) {
        if ($owned) { try { $application.Quit() } catch {} }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($application)
    }
}
