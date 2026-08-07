<#
.SYNOPSIS
    Draws the 1280x640 GitHub social-preview banner: the SpiritMark lockup
    on the grove night ground.

.DESCRIPTION
    Companion to docs/SPIRIT_MARK.md and scripts/generate-app-icon.ps1, whose
    disc recipe (paper fill, caught-light sheen, fern rim straddling the fill
    edge) and colors this script mirrors. It renders the horizontal lockup
    from design/spirit-mark.html: the idle mark, then the lowercase kodabi
    wordmark, proportioned per the page (gap 0.4x the core diameter, wordmark
    em 0.775x) and centered on the banner. Clear space follows the mark's ma
    rule: at least the core's own diameter on every side; the script warns if
    a margin falls short.

    The mark is the idle ink form: green means audio is being recorded and is
    spent nowhere else (SPIRIT_MARK.md "Trust / consent signal"), so the only
    green here is the faint grove ground glow the app itself wears, never the
    disc. Colors are hard-coded because a PNG cannot read a CSS custom
    property, and are their own source of truth:
      #E9E8DE  --k-paper        disc fill and wordmark ink
      #3B4636  --k-fern         rim
      #111710  --k-night        Grove night ground
      rgba(255,255,255,.16)     --sheen
      rgba(150,206,124,.12)     grove ground glow, green
      rgba(231,179,74,.07)      grove ground glow, warm

.NOTES
    The output is committed as assets/brand/social-preview.png. GitHub reads
    it nowhere automatically: after a change, upload it by hand at
    Settings > General > Social preview. The README hero may also embed it.

.EXAMPLE
    .\scripts\generate-social-preview.ps1
#>

[CmdletBinding()]
param(
    # Where the banner lands.
    [string] $OutPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Defaults resolve here, not in the param block: Windows PowerShell 5.1
# leaves $PSScriptRoot empty while binding an advanced script's defaults.
if (-not $OutPath) {
    $OutPath = Join-Path $PSScriptRoot '..\assets\brand\social-preview.png'
}

Add-Type -AssemblyName System.Drawing

# --- Tunables ---------------------------------------------------------------
# Geometry is expressed in final-pixel units and scaled up for drawing.
$Width = 1280         # GitHub's recommended social-preview size
$Height = 640
$Supersample = 2      # 2560x1280 intermediate is plenty at this scale
$CoreDiameter = 196.0 # D: the mark's core; every lockup measure derives from it
$GapRatio = 0.4       # gap between mark and wordmark, x D (spirit-mark.html)
$WordmarkRatio = 0.775 # wordmark em size, x D (spirit-mark.html)
$TextNudge = 0.0      # optical vertical correction for the wordmark, final px
$RimRatio = 1.9 / 19.0 # rim stroke : disc diameter, the tray's proportion
$SheenAlpha = 41      # --sheen: 0.16 * 255
$SheenCenterX = 0.42  # sheen center, as fractions of the disc bounds
$SheenCenterY = 0.38
$SheenRadius = 0.52   # sheen fade radius as a fraction of the disc diameter
$GlowGreenRadius = 0.85 # green glow radius as a fraction of banner width
$GlowWarmRadius = 0.70  # warm glow radius as a fraction of banner width

$Paper = [System.Drawing.Color]::FromArgb(0xE9, 0xE8, 0xDE)
$Fern = [System.Drawing.Color]::FromArgb(0x3B, 0x46, 0x36)
$Night = [System.Drawing.Color]::FromArgb(0x11, 0x17, 0x10)
$GlowGreen = [System.Drawing.Color]::FromArgb(31, 0x96, 0xCE, 0x7C)
$GlowWarm = [System.Drawing.Color]::FromArgb(18, 0xE7, 0xB3, 0x4A)

# --- Helpers (mirroring generate-app-icon.ps1) ------------------------------

# A square of the given diameter centered on a point, in supersampled units.
function New-DiscRect {
    param([double] $Diameter, [double] $CenterX, [double] $CenterY)

    $side = $Diameter * $Supersample
    [System.Drawing.RectangleF]::new(
        $CenterX * $Supersample - $side / 2.0,
        $CenterY * $Supersample - $side / 2.0,
        $side, $side)
}

# The idle mark: paper fill, caught-light sheen, fern rim.
function Add-SpiritDisc {
    param([System.Drawing.Graphics] $Graphics, [System.Drawing.RectangleF] $Bounds)

    $brush = [System.Drawing.SolidBrush]::new($Paper)
    try { $Graphics.FillEllipse($brush, $Bounds) } finally { $brush.Dispose() }

    $sheenSide = $Bounds.Width * $SheenRadius * 2.0
    $sheenRect = [System.Drawing.RectangleF]::new(
        $Bounds.X + $Bounds.Width * $SheenCenterX - $sheenSide / 2.0,
        $Bounds.Y + $Bounds.Height * $SheenCenterY - $sheenSide / 2.0,
        $sheenSide, $sheenSide)
    $sheenPath = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $discPath = [System.Drawing.Drawing2D.GraphicsPath]::new()
    try {
        $sheenPath.AddEllipse($sheenRect)
        $discPath.AddEllipse($Bounds)
        $gradient = [System.Drawing.Drawing2D.PathGradientBrush]::new($sheenPath)
        try {
            $gradient.CenterColor = [System.Drawing.Color]::FromArgb($SheenAlpha, 255, 255, 255)
            $gradient.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 255, 255, 255))
            $Graphics.SetClip($discPath)
            try { $Graphics.FillEllipse($gradient, $sheenRect) } finally { $Graphics.ResetClip() }
        } finally { $gradient.Dispose() }
    } finally {
        $discPath.Dispose()
        $sheenPath.Dispose()
    }

    $pen = [System.Drawing.Pen]::new($Fern, [float]($Bounds.Width * $RimRatio))
    try { $Graphics.DrawEllipse($pen, $Bounds) } finally { $pen.Dispose() }
}

# One grove ground glow: a radial wash fading to nothing.
function Add-Glow {
    param(
        [System.Drawing.Graphics] $Graphics,
        [double] $CenterX, [double] $CenterY, [double] $Radius,
        [System.Drawing.Color] $Color
    )

    $rect = New-DiscRect -Diameter ($Radius * 2.0) -CenterX $CenterX -CenterY $CenterY
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    try {
        $path.AddEllipse($rect)
        $gradient = [System.Drawing.Drawing2D.PathGradientBrush]::new($path)
        try {
            $gradient.CenterColor = $Color
            $gradient.SurroundColors = @([System.Drawing.Color]::FromArgb(0, $Color))
            $Graphics.FillEllipse($gradient, $rect)
        } finally { $gradient.Dispose() }
    } finally { $path.Dispose() }
}

# The wordmark face: Bahnschrift SemiBold where Windows exposes the variable
# font's named instance to GDI+, otherwise Bahnschrift emboldened.
function Resolve-WordmarkFont {
    param([double] $EmPixels)

    foreach ($candidate in @(
            @{ Family = 'Bahnschrift SemiBold'; Style = [System.Drawing.FontStyle]::Regular },
            @{ Family = 'Bahnschrift'; Style = [System.Drawing.FontStyle]::Bold })) {
        try {
            $family = [System.Drawing.FontFamily]::new($candidate.Family)
            return [System.Drawing.Font]::new(
                $family, [float] $EmPixels, $candidate.Style,
                [System.Drawing.GraphicsUnit]::Pixel)
        } catch {
            Write-Verbose "font '$($candidate.Family)' unavailable, falling back"
        }
    }
    throw 'Bahnschrift is not installed; it ships with Windows 10 and later.'
}

# --- The banner -------------------------------------------------------------

$outDir = Split-Path $OutPath -Parent
if (-not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir | Out-Null
}

$large = [System.Drawing.Bitmap]::new(
    $Width * $Supersample, $Height * $Supersample,
    [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$small = [System.Drawing.Bitmap]::new($Width, $Height,
    [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
try {
    $graphics = [System.Drawing.Graphics]::FromImage($large)
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
        $graphics.Clear($Night)

        # The grove-ground washes, as src/index.css @utility grove-ground
        # places them: green from the top left, warm from the bottom right.
        Add-Glow -Graphics $graphics -CenterX ($Width * 0.08) -CenterY ($Height * -0.10) `
            -Radius ($Width * $GlowGreenRadius) -Color $GlowGreen
        Add-Glow -Graphics $graphics -CenterX $Width -CenterY ($Height * 1.10) `
            -Radius ($Width * $GlowWarmRadius) -Color $GlowWarm

        # Lockup layout, in final units: mark, gap, wordmark, centered as a
        # unit. The rim straddles the disc edge, so the visual lockup is a
        # touch wider than D; the ma margins are computed against that.
        $font = Resolve-WordmarkFont -EmPixels ($WordmarkRatio * $CoreDiameter * $Supersample)
        try {
            $format = [System.Drawing.StringFormat]::GenericTypographic
            $textSize = $graphics.MeasureString('kodabi', $font, [System.Drawing.PointF]::Empty, $format)
            $textW = $textSize.Width / $Supersample
            $textH = $textSize.Height / $Supersample

            $rim = $CoreDiameter * $RimRatio / 2.0   # rim overhang past the fill
            $gap = $GapRatio * $CoreDiameter
            $lockupW = $rim + $CoreDiameter + $rim + $gap + $textW
            $left = ($Width - $lockupW) / 2.0

            $sideMargin = $left - $rim
            $topMargin = ($Height - $CoreDiameter) / 2.0 - $rim
            foreach ($margin in @($sideMargin, $topMargin)) {
                if ($margin -lt $CoreDiameter) {
                    Write-Warning ("ma clear space violated: margin {0:n0}px < core {1:n0}px" `
                            -f $margin, $CoreDiameter)
                }
            }

            $bounds = New-DiscRect -Diameter $CoreDiameter `
                -CenterX ($left + $rim + $CoreDiameter / 2.0) -CenterY ($Height / 2.0)
            Add-SpiritDisc -Graphics $graphics -Bounds $bounds

            $brush = [System.Drawing.SolidBrush]::new($Paper)
            try {
                $graphics.DrawString('kodabi', $font, $brush,
                    [System.Drawing.PointF]::new(
                        ($left + $rim + $CoreDiameter + $rim + $gap) * $Supersample,
                        ($Height / 2.0 - $textH / 2.0 + $TextNudge) * $Supersample),
                    $format)
            } finally { $brush.Dispose() }
        } finally { $font.Dispose() }
    } finally { $graphics.Dispose() }

    $shrink = [System.Drawing.Graphics]::FromImage($small)
    try {
        $shrink.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $shrink.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $shrink.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $shrink.DrawImage($large, [System.Drawing.Rectangle]::new(0, 0, $Width, $Height))
    } finally { $shrink.Dispose() }

    $small.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
    Write-Host "wrote $OutPath"
} finally {
    $small.Dispose()
    $large.Dispose()
}
