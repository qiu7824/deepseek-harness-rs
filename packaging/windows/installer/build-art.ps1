# Regenerate the installer artwork (24-bit BMPs, composited on white, since
# Inno Setup 6.1.2 bitmap controls take BMP only). Renders with headless
# Chrome at 1x and 2x so the installer can pick the sharper one per DPI.
param(
  [string]$Chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$here = $PSScriptRoot
$root = Resolve-Path (Join-Path $here '..\..\..')
$work = Join-Path ([System.IO.Path]::GetTempPath()) ('dsh-installer-art-' + [guid]::NewGuid())
New-Item -ItemType Directory -Force $work | Out-Null
try {
  $whale = [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $root 'packaging\windows\deepseek-black.png')))
  # The brand SVG starts with the whale; the wordmark shows the text and badge only.
  $brand = (Get-Content -Raw -Encoding UTF8 (Join-Path $root 'apps\desktop_flutter\assets\brand.svg')) `
    -replace 'currentColor', '#000' -replace 'viewBox="0 0 182 24"', 'viewBox="26 0 156 24"' `
    -replace 'width="182" height="24"', 'width="100%" height="100%"'
  $pieces = @{
    # 112 px light-gray disc with the black whale, as in the product mockup.
    'logo' = @{ Width = 112; Height = 112; Body = "<div style='width:112px;height:112px;border-radius:50%;background:#f1f2f4;display:flex;align-items:center;justify-content:center'><img src='data:image/png;base64,$whale' style='width:62px;height:62px'></div>" }
    'wordmark' = @{ Width = 208; Height = 32; Body = "<div style='width:208px;height:32px'>$brand</div>" }
    # Rounded black primary button; its caption is a localized label on top.
    'button' = @{ Width = 220; Height = 44; Body = "<div style='width:220px;height:44px;border-radius:12px;background:#111'></div>" }
  }
  foreach ($name in $pieces.Keys) {
    $piece = $pieces[$name]
    $html = Join-Path $work "$name.html"
    Set-Content -Encoding UTF8 $html "<!doctype html><html><body style='margin:0;background:#fff;overflow:hidden'>$($piece.Body)</body></html>"
    foreach ($scale in 1, 2) {
      $png = Join-Path $work "$name-$scale.png"
      & $Chrome --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=$scale `
        "--screenshot=$png" "--window-size=$($piece.Width),$($piece.Height)" ([Uri]$html).AbsoluteUri 2>$null | Out-Null
      if (-not (Test-Path $png)) { throw "Chrome did not render $name at ${scale}x" }
      $source = [System.Drawing.Image]::FromFile($png)
      try {
        $bitmap = New-Object System.Drawing.Bitmap($source.Width, $source.Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.Clear([System.Drawing.Color]::White)
        $graphics.DrawImage($source, 0, 0, $source.Width, $source.Height)
        $graphics.Dispose()
        $bitmap.Save((Join-Path $here "$name-${scale}x.bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp)
        $bitmap.Dispose()
      } finally {
        $source.Dispose()
      }
    }
  }
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
