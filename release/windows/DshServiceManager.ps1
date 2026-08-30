#requires -Version 7.0
param([int]$Port = 58080)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$script:ScriptRoot = Split-Path -Parent $PSCommandPath
$script:Root = if (Test-Path (Join-Path $script:ScriptRoot 'deepseek harness-rs.exe')) { $script:ScriptRoot } else { Split-Path -Parent $script:ScriptRoot }
$script:Exe = Join-Path $script:Root 'deepseek harness-rs.exe'
$script:RunDir = Join-Path $script:Root 'run'
$script:LogDir = Join-Path $script:Root 'logs'
$script:PidFile = Join-Path $script:RunDir 'dsh.pid'
$script:OutLog = Join-Path $script:LogDir 'dsh.out.log'
$script:ErrLog = Join-Path $script:LogDir 'dsh.err.log'
New-Item -ItemType Directory -Force -Path $script:RunDir, $script:LogDir | Out-Null

function Get-ListeningPid {
    try {
        $row = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction Stop | Select-Object -First 1
        if ($null -ne $row) { return [int]$row.OwningProcess }
    } catch {}
    return 0
}

function Test-ServicePort {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $pending = $client.BeginConnect('127.0.0.1', $Port, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne(300)) { return $false }
        $client.EndConnect($pending)
        return $true
    } catch { return $false } finally { $client.Dispose() }
}

function Get-ServiceState {
    if (-not (Test-ServicePort)) { return @{ Running = $false; Pid = 0; Detail = '服务未运行' } }
    return @{ Running = $true; Pid = 0; Detail = "DSH Web 正在运行 · 端口 $Port" }
}

function Start-Dsh {
    if (-not (Test-Path -LiteralPath $script:Exe)) { throw "未找到 $script:Exe" }
    $state = Get-ServiceState
    if ($state.Running) { return }
    $process = Start-Process -FilePath $script:Exe -ArgumentList @('web','--port',"$Port") -WorkingDirectory $script:Root -RedirectStandardOutput $script:OutLog -RedirectStandardError $script:ErrLog -WindowStyle Hidden -PassThru
    Set-Content -LiteralPath $script:PidFile -Value $process.Id -Encoding ascii
}

function Stop-Dsh {
    $svcPid = Get-ListeningPid
    if ($svcPid -le 0) { Remove-Item $script:PidFile -Force -ErrorAction SilentlyContinue; return }
    $process = Get-CimInstance Win32_Process -Filter "ProcessId=$svcPid"
    $path = [IO.Path]::GetFullPath([string]$process.ExecutablePath)
    if (-not $path.Equals([IO.Path]::GetFullPath($script:Exe), [StringComparison]::OrdinalIgnoreCase)) {
        throw "拒绝停止：端口 $Port 不是由当前目录的 dsh.exe 监听"
    }
    Stop-Process -Id $svcPid -Force
    Remove-Item $script:PidFile -Force -ErrorAction SilentlyContinue
}

$script:TrayIcon = [System.Windows.Forms.NotifyIcon]::new()
$script:TrayIcon.Text = 'DeepSeek Harness-rs'
$script:TrayIcon.Icon = [System.Drawing.SystemIcons]::Application
$script:TrayIcon.Visible = $true
$script:TrayMenu = [System.Windows.Forms.ContextMenuStrip]::new()
$script:TrayStatusItem = $script:TrayMenu.Items.Add('正在检测服务状态')
$script:TrayStatusItem.Enabled = $false
$script:TrayStartItem = $script:TrayMenu.Items.Add('启动服务')
$script:TrayStopItem = $script:TrayMenu.Items.Add('停止服务')
$script:TrayOpenItem = $script:TrayMenu.Items.Add('打开网页')
[void]$script:TrayMenu.Items.Add('-')
$script:TrayShowItem = $script:TrayMenu.Items.Add('显示运行管理器')
$script:TrayExitItem = $script:TrayMenu.Items.Add('退出托盘')
$script:TrayIcon.ContextMenuStrip = $script:TrayMenu

[xml]$xaml = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" Title="DeepSeek Harness-rs 运行管理器" Width="720" Height="500" MinWidth="620" MinHeight="440" WindowStartupLocation="CenterScreen" Background="#0B0D10" FontFamily="Microsoft YaHei UI">
  <Window.Resources>
    <Style TargetType="Button"><Setter Property="Height" Value="40"/><Setter Property="Padding" Value="18,0"/><Setter Property="Margin" Value="0,0,10,10"/><Setter Property="Foreground" Value="#F7F8F8"/><Setter Property="Background" Value="#191B20"/><Setter Property="BorderBrush" Value="#343840"/><Setter Property="Cursor" Value="Hand"/></Style>
  </Window.Resources>
  <Grid Margin="24"><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
    <StackPanel><TextBlock Text="DeepSeek Harness-rs" FontSize="28" FontWeight="SemiBold" Foreground="#F7F8F8"/><TextBlock Text="Rust 原生 Web Agent · 正式运行入口" Margin="0,6,0,20" Foreground="#8A8F98"/></StackPanel>
    <Border Grid.Row="1" Background="#111318" BorderBrush="#2B2F36" BorderThickness="1" CornerRadius="12" Padding="20"><StackPanel><TextBlock x:Name="StatusTitle" FontSize="19" FontWeight="SemiBold"/><TextBlock x:Name="StatusDetail" Margin="0,8,0,0" Foreground="#AAB0BA"/><TextBlock x:Name="Address" Margin="0,8,0,0" Text="http://127.0.0.1:58080/" Foreground="#7AA2F7"/></StackPanel></Border>
    <WrapPanel Grid.Row="2" Margin="0,18,0,18"><Button x:Name="StartButton" Content="启动" Background="#175CD3" Foreground="White"/><Button x:Name="StopButton" Content="停止"/><Button x:Name="RestartButton" Content="重启"/><Button x:Name="OpenButton" Content="打开网页"/><Button x:Name="LogButton" Content="日志目录"/></WrapPanel>
    <TextBox Grid.Row="3" x:Name="LogBox" IsReadOnly="True" TextWrapping="Wrap" VerticalScrollBarVisibility="Auto" Background="#050607" Foreground="#C9D1D9" BorderBrush="#2B2F36" BorderThickness="1" Padding="14" FontFamily="Consolas"/>
  </Grid>
</Window>
'@
$window = [Windows.Markup.XamlReader]::Load((New-Object System.Xml.XmlNodeReader $xaml))
$ui = @{}
'StatusTitle','StatusDetail','Address','StartButton','StopButton','RestartButton','OpenButton','LogButton','LogBox' | ForEach-Object { $ui[$_] = $window.FindName($_) }

function Write-UiLog([string]$text) { $ui.LogBox.AppendText("$(Get-Date -Format HH:mm:ss)  $text`r`n"); $ui.LogBox.ScrollToEnd() }
function Refresh-State {
    $state = Get-ServiceState
    $ui.StatusTitle.Text = if ($state.Running) { '运行中' } else { '已停止' }
    $ui.StatusTitle.Foreground = if ($state.Running) { '#69D28A' } else { '#FF6E6E' }
    $ui.StatusDetail.Text = $state.Detail
    $ui.StartButton.IsEnabled = -not $state.Running
    $ui.StopButton.IsEnabled = $state.Running
    $ui.RestartButton.IsEnabled = $state.Running
    $script:TrayStatusItem.Text = if ($state.Running) { "运行中 · 端口 $Port" } else { '服务已停止' }
    $script:TrayStartItem.Enabled = -not $state.Running
    $script:TrayStopItem.Enabled = $state.Running
    $script:TrayIcon.Text = if ($state.Running) { "DeepSeek Harness-rs · 运行中 ($Port)" } else { 'DeepSeek Harness-rs · 已停止' }
}
function Invoke-Safe([scriptblock]$action, [string]$ok) { try { & $action; Start-Sleep -Milliseconds 350; Refresh-State; Write-UiLog $ok } catch { Write-UiLog "错误：$($_.Exception.Message)"; Refresh-State } }
$ui.StartButton.Add_Click({ Invoke-Safe { Start-Dsh } '启动命令已执行' })
$ui.StopButton.Add_Click({ Invoke-Safe { Stop-Dsh } '停止命令已执行' })
$ui.RestartButton.Add_Click({ Invoke-Safe { Stop-Dsh; Start-Sleep -Milliseconds 300; Start-Dsh } '重启命令已执行' })
$ui.OpenButton.Add_Click({ Start-Process "http://127.0.0.1:$Port/" })
$ui.LogButton.Add_Click({ Start-Process explorer.exe $script:LogDir })
$script:TrayStartItem.Add_Click({ Invoke-Safe { Start-Dsh } '托盘启动命令已执行' })
$script:TrayStopItem.Add_Click({ Invoke-Safe { Stop-Dsh } '托盘停止命令已执行' })
$script:TrayOpenItem.Add_Click({ Start-Process "http://127.0.0.1:$Port/" })
$script:TrayShowItem.Add_Click({ $window.Show(); $window.Activate() })
$script:TrayIcon.Add_DoubleClick({ $window.Show(); $window.Activate() })
$script:TrayExitItem.Add_Click({
    $script:TrayIcon.Visible = $false
    $script:TrayIcon.Dispose()
    $window.Tag = 'exit'
    $window.Close()
})
$window.Add_Closing({
    param($sender, $eventArgs)
    if ($window.Tag -ne 'exit') {
        $eventArgs.Cancel = $true
        $window.Hide()
        $script:TrayIcon.ShowBalloonTip(1500, 'DeepSeek Harness-rs', '运行管理器已缩小到右下角托盘。', [System.Windows.Forms.ToolTipIcon]::Info)
    }
})
$timer = [System.Windows.Threading.DispatcherTimer]::new(); $timer.Interval = [TimeSpan]::FromSeconds(2); $timer.Add_Tick({ Refresh-State }); $timer.Start()
$ui.StatusTitle.Text = '正在检测'
$ui.StatusDetail.Text = "服务入口：deepseek harness-rs.exe web --port $Port"
Write-UiLog "管理器已就绪；正在检测服务状态"
$window.Add_ContentRendered({ Refresh-State })
[void]$window.ShowDialog()
$timer.Stop()
$script:TrayIcon.Visible = $false
$script:TrayIcon.Dispose()
