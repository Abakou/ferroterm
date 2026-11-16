# Génère l'icône de Ferroterm : PNG multi-tailles, fichier .ico et données RGBA
# brutes pour l'icône de fenêtre. À relancer après modification du dessin :
#   powershell -ExecutionPolicy Bypass -File assets\make_icon.ps1
Add-Type -AssemblyName System.Drawing
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path

function Draw-Icon([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'
    $s = $size / 256.0
    $g.ScaleTransform($s, $s)

    # Fenêtre de terminal : carré arrondi sombre.
    $rect = New-Object System.Drawing.RectangleF 8, 8, 240, 240
    $radius = 58
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($rect.X, $rect.Y, $radius, $radius, 180, 90)
    $path.AddArc($rect.Right - $radius, $rect.Y, $radius, $radius, 270, 90)
    $path.AddArc($rect.Right - $radius, $rect.Bottom - $radius, $radius, $radius, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $radius, $radius, $radius, 90, 90)
    $path.CloseFigure()
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, 0), (New-Object System.Drawing.PointF 0, 256), ([System.Drawing.Color]::FromArgb(255, 0x20, 0x25, 0x2e)), ([System.Drawing.Color]::FromArgb(255, 0x0f, 0x12, 0x17))
    $g.FillPath($brush, $path)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(60, 255, 255, 255)), 3
    $g.DrawPath($pen, $path)

    # Invite « $ » verte (shell Unix), suivie d'un chevron « > » clair.
    $green = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x3d, 0xdc, 0x97))
    $glyph = New-Object System.Drawing.Drawing2D.GraphicsPath
    $family = New-Object System.Drawing.FontFamily 'Consolas'
    $glyph.AddString('$', $family, [int][System.Drawing.FontStyle]::Bold, 132, (New-Object System.Drawing.PointF 44, 50), [System.Drawing.StringFormat]::GenericTypographic)
    $g.FillPath($green, $glyph)
    $white = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 0xe6, 0xea, 0xf2)), 22
    $white.StartCap = 'Round'; $white.EndCap = 'Round'; $white.LineJoin = 'Round'
    $chevron = [System.Drawing.PointF[]]@((New-Object System.Drawing.PointF 146, 88), (New-Object System.Drawing.PointF 192, 128), (New-Object System.Drawing.PointF 146, 168))
    $g.DrawLines($white, $chevron)

    $g.Dispose()
    return $bmp
}

$sizes = 256, 128, 64, 48, 32, 16
$pngs = @{}
foreach ($size in $sizes) {
    $bmp = Draw-Icon $size
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs[$size] = $ms.ToArray()
    if ($size -eq 256) { $bmp.Save("$dir\ferroterm-256.png", [System.Drawing.Imaging.ImageFormat]::Png) }
    if ($size -eq 64) {
        # RGBA brut pour l'icône de fenêtre (egui::IconData).
        $raw = New-Object byte[] ($size * $size * 4); $i = 0
        for ($y = 0; $y -lt $size; $y++) { for ($x = 0; $x -lt $size; $x++) { $c = $bmp.GetPixel($x, $y); $raw[$i] = $c.R; $raw[$i+1] = $c.G; $raw[$i+2] = $c.B; $raw[$i+3] = $c.A; $i += 4 } }
        [System.IO.File]::WriteAllBytes("$dir\ferroterm-64.rgba", $raw)
    }
    $bmp.Dispose(); $ms.Dispose()
}

# Fichier ICO : en-tête, répertoire, puis les PNG (acceptés par Windows pour toutes les tailles).
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($size in $sizes) {
    $data = $pngs[$size]
    $w.Write([byte]$(if ($size -ge 256) { 0 } else { $size })); $w.Write([byte]$(if ($size -ge 256) { 0 } else { $size }))
    $w.Write([byte]0); $w.Write([byte]0); $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$data.Length); $w.Write([uint32]$offset)
    $offset += $data.Length
}
foreach ($size in $sizes) { $w.Write($pngs[$size]) }
$w.Flush()
[System.IO.File]::WriteAllBytes("$dir\ferroterm.ico", $out.ToArray())
"ferroterm.ico : $($out.Length) octets, tailles $($sizes -join ', ')"
