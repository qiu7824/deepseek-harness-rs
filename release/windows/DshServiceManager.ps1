#requires -Version 7.0
param([int]$Port = 58080)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase

$script:ScriptRoot = Split-Path -Parent $PSCommandPath
$script:Root = if (Test-Path (Join-Path $script:ScriptRoot 'dsh.exe')) { $script:ScriptRoot } else { Split-Path -Parent $script:ScriptRoot }
$script:Exe = Join-Path $script:Root 'dsh.exe'
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

function Get-ServiceState {
    $svcPid = Get-ListeningPid
    if ($svcPid -le 0) { return @{ Running = $false; Pid = 0; Detail = '服务未运行' } }
    try {
        $process = Get-CimInstance Win32_Process -Filter "ProcessId=$svcPid"
        $path = [IO.Path]::GetFullPath([string]$process.ExecutablePath)
        $expected = [IO.Path]::GetFullPath($script:Exe)
        if (-not $path.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) {
            return @{ Running = $true; Pid = $svcPid; Detail = "端口 $Port 被其他程序占用" }
        }
    } catch { return @{ Running = $true; Pid = $svcPid; Detail = "端口 $Port 正在监听" } }
    return @{ Running = $true; Pid = $svcPid; Detail = "DSH Web 正在运行 · PID $svcPid" }
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

[xml]$xaml = @'
<Window xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Title="DeepSeek Harness 运行管理器" Width="620" Height="430" MinWidth="560" MinHeight="390" WindowStartupLocation="CenterScreen" Background="#F5F7FA" FontFamily="Microsoft YaHei UI">
  <Window.Resources>
    <Style TargetType="Button"><Setter Property="Height" Value="38"/><Setter Property="Padding" Value="18,0"/><Setter Property="Margin" Value="0,0,10,0"/><Setter Property="Background" Value="#FFFFFF"/><Setter Property="BorderBrush" Value="#D7DCE3"/><Setter Property="Cursor" Value="Hand"/></Style>
  </Window.Resources>
  <Grid Margin="24"><Grid.RowDefinitions><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="Auto"/><RowDefinition Height="*"/></Grid.RowDefinitions>
    <StackPanel><TextBlock Text="DeepSeek Harness" FontSize="26" FontWeight="SemiBold" Foreground="#172033"/><TextBlock Text="正式 Web 服务运行管理" Margin="0,6,0,20" Foreground="#667085"/></StackPanel>
    <Border Grid.Row="1" Background="White" BorderBrush="#E1E5EA" BorderThickness="1" CornerRadius="10" Padding="18"><StackPanel><TextBlock x:Name="StatusTitle" FontSize="18" FontWeight="SemiBold"/><TextBlock x:Name="StatusDetail" Margin="0,8,0,0" Foreground="#667085"/><TextBlock x:Name="Address" Margin="0,6,0,0" Text="http://127.0.0.1:58080/" Foreground="#175CD3"/></StackPanel></Border>
    <WrapPanel Grid.Row="2" Margin="0,18,0,18"><Button x:Name="StartButton" Content="启动" Background="#175CD3" Foreground="White"/><Button x:Name="StopButton" Content="停止"/><Button x:Name="RestartButton" Content="重启"/><Button x:Name="OpenButton" Content="打开网页"/><Button x:Name="LogButton" Content="日志目录"/></WrapPanel>
    <TextBox Grid.Row="3" x:Name="LogBox" IsReadOnly="True" TextWrapping="Wrap" VerticalScrollBarVisibility="Auto" Background="#101828" Foreground="#D0D5DD" BorderThickness="0" Padding="14" FontFamily="Consolas"/>
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
    $ui.StatusTitle.Foreground = if ($state.Running) { '#027A48' } else { '#B42318' }
    $ui.StatusDetail.Text = $state.Detail
    $ui.StartButton.IsEnabled = -not $state.Running
    $ui.StopButton.IsEnabled = $state.Running
    $ui.RestartButton.IsEnabled = $state.Running
}
function Invoke-Safe([scriptblock]$action, [string]$ok) { try { & $action; Start-Sleep -Milliseconds 350; Refresh-State; Write-UiLog $ok } catch { Write-UiLog "错误：$($_.Exception.Message)"; Refresh-State } }
$ui.StartButton.Add_Click({ Invoke-Safe { Start-Dsh } '启动命令已执行' })
$ui.StopButton.Add_Click({ Invoke-Safe { Stop-Dsh } '停止命令已执行' })
$ui.RestartButton.Add_Click({ Invoke-Safe { Stop-Dsh; Start-Sleep -Milliseconds 300; Start-Dsh } '重启命令已执行' })
$ui.OpenButton.Add_Click({ Start-Process "http://127.0.0.1:$Port/" })
$ui.LogButton.Add_Click({ Start-Process explorer.exe $script:LogDir })
$timer = [System.Windows.Threading.DispatcherTimer]::new(); $timer.Interval = [TimeSpan]::FromSeconds(2); $timer.Add_Tick({ Refresh-State }); $timer.Start()
Refresh-State
Write-UiLog "管理器已就绪；服务入口为 dsh.exe web --port $Port"
[void]$window.ShowDialog()
$timer.Stop()
