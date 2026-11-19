# Generates assets/icon.png (256x256) used for the tray icon.
Add-Type -AssemblyName System.Drawing

$size = 256
$bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

$rect = New-Object System.Drawing.Rectangle(8, 8, ($size - 16), ($size - 16))
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$r = 56
$path.AddArc($rect.X, $rect.Y, $r, $r, 180, 90)
$path.AddArc($rect.Right - $r, $rect.Y, $r, $r, 270, 90)
$path.AddArc($rect.Right - $r, $rect.Bottom - $r, $r, $r, 0, 90)
$path.AddArc($rect.X, $rect.Bottom - $r, $r, $r, 90, 90)
$path.CloseFigure()

$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($size, $size)),
    [System.Drawing.Color]::FromArgb(255, 47, 107, 255),
    [System.Drawing.Color]::FromArgb(255, 18, 194, 178))
$g.FillPath($brush, $path)

$white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
$bars = @(
    @{ X = 66; H = 78 },
    @{ X = 110; H = 128 },
    @{ X = 154; H = 96 }
)
foreach ($bar in $bars) {
    $w = 36
    $y = 128 - ($bar.H / 2)
    $barRect = New-Object System.Drawing.Rectangle($bar.X, [int]$y, $w, $bar.H)
    $barPath = New-Object System.Drawing.Drawing2D.GraphicsPath
    $br = 18
    $barPath.AddArc($barRect.X, $barRect.Y, $br, $br, 180, 90)
    $barPath.AddArc($barRect.Right - $br, $barRect.Y, $br, $br, 270, 90)
    $barPath.AddArc($barRect.Right - $br, $barRect.Bottom - $br, $br, $br, 0, 90)
    $barPath.AddArc($barRect.X, $barRect.Bottom - $br, $br, $br, 90, 90)
    $barPath.CloseFigure()
    $g.FillPath($white, $barPath)
    $barPath.Dispose()
}

New-Item -ItemType Directory -Force (Join-Path $PSScriptRoot "..\assets") | Out-Null
$out = Join-Path $PSScriptRoot "..\assets\icon.png"
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
$brush.Dispose()
$white.Dispose()
Write-Output $out
