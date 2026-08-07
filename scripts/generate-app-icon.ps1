<#
.SYNOPSIS
    Draws the SpiritMark app-icon master and the NSIS installer wizard art.

.DESCRIPTION
    Companion to docs/SPIRIT_MARK.md and to scripts/generate-tray-icons.ps1,
    whose drawing pipeline (supersample, bicubic downscale) and colors this
    script shares. It produces the branding the OS shows outside the app:

      assets/brand/app-icon-master.png   1024px master; `pnpm tauri icon`
                                         derives the whole src-tauri/icons
                                         set (taskbar, Alt-Tab, Start menu,
                                         installer, Add/Remove Programs)
      src-tauri/icons/nsis/header.bmp    150x57 NSIS wizard header strip
      src-tauri/icons/nsis/sidebar.bmp   164x314 NSIS welcome/finish sidebar

    The mark is the idle ink form only: paper disc, fern rim, and the faint
    caught-light sheen from design/spirit-mark.html. Green means audio is
    being recorded and is spent nowhere else (SPIRIT_MARK.md "Trust / consent
    signal"), so the mark never wears it on a static OS surface; the only
    green below is the faint grove ground glow the app itself wears, on the
    sidebar backdrop only.

    Geometry differs from the tray in one way: the disc takes 0.72 of the
    canvas instead of the tray's 19/32, because the static icon never shows
    the recording aura and so reserves no room for it. The rim keeps the
    tray's rim:disc proportion, and like the tray's Add-Rim the stroke
    straddles the fill edge.

    Colors are hard-coded because a PNG cannot read a CSS custom property,
    and are their own source of truth (the OS shell does not follow the
    in-app theme):
      #E9E8DE  --k-paper        the ink treatment, inverted
      #3B4636  --k-fern         rim, so the mark also reads on light grounds
      #111710  --k-night        Grove night ground, the NSIS art backdrop
      rgba(255,255,255,.16)     --sheen
      rgba(150,206,124,.12)     grove ground glow, green (sidebar only)
      rgba(231,179,74,.07)      grove ground glow, warm (sidebar only)

.NOTES
    After regenerating the master, derive the icon set from the repo root:

      pnpm tauri icon assets/brand/app-icon-master.png

    It writes into src-tauri/icons/ (delete any stray android/ or ios/
    subdirectories it emits; nothing references them). tauri's
    generate_context! embeds the icons at compile time and cargo does not
    track them as dependencies, so after regenerating touch
    src-tauri/build.rs (or `cargo clean -p kodabi`) or a local rebuild will
    keep the stale embedding.

    The NSIS images must stay 24bpp BMP: the installer ignores alpha, and a
    32bpp BMP renders with artifacts in the wizard.

.EXAMPLE
    .\scripts\generate-app-icon.ps1
#>

[CmdletBinding()]
param(
    # Where the 1024px master lands; `pnpm tauri icon` reads it from here.
    [string] $MasterPath,
    # Where the NSIS wizard BMPs land; tauri.conf.json bundle.windows.nsis
    # points here.
    [string] $NsisDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Defaults resolve here, not in the param block: Windows PowerShell 5.1
# leaves $PSScriptRoot empty while binding an advanced script's defaults.
if (-not $MasterPath) {
    $MasterPath = Join-Path $PSScriptRoot '..\assets\brand\app-icon-master.png'
}
if (-not $NsisDir) {
    $NsisDir = Join-Path $PSScriptRoot '..\src-tauri\icons\nsis'
}

Add-Type -AssemblyName System.Drawing

# --- Tunables ---------------------------------------------------------------
# Geometry is expressed in final-pixel units and scaled up for drawing. The
# disc numbers are the "variant B" pick from the icon variant sheet.
$Size = 1024          # master canvas; every shipped size derives from it
$Supersample = 4      # 4096px intermediate; 8x would be a 268 MB bitmap for
                      # no visible gain at this size
$DiscFraction = 0.72  # disc diameter as a fraction of the canvas
$RimRatio = 1.9 / 19.0 # rim stroke : disc diameter, the tray's proportion
$SheenAlpha = 41      # --sheen: 0.16 * 255
$SheenCenterX = 0.42  # sheen center, as fractions of the disc bounds
$SheenCenterY = 0.38  # (design page: radial-gradient circle at 42% 38%)
$SheenRadius = 0.52   # sheen fade radius as a fraction of the disc diameter

# NSIS wizard art. The pixel sizes are fixed by the NSIS Modern UI, not ours
# to tune; the disc sizes and spacing are.
$HeaderW = 150; $HeaderH = 57
$HeaderDisc = 38.0     # header mark diameter
$SidebarW = 164; $SidebarH = 314
$SidebarDisc = 72.0    # sidebar mark diameter
$SidebarGap = 22.0     # gap between disc bottom and wordmark top
$SidebarWordmarkEm = 30.0 # wordmark font size; the lockup here is stacked,
                          # so the horizontal lockup's 0.775 ratio would
                          # overflow the 164px panel
$GlowGreenRadius = 0.75 # green glow radius as a fraction of sidebar height
$GlowWarmRadius = 0.60  # warm glow radius as a fraction of sidebar height

$Paper = [System.Drawing.Color]::FromArgb(0xE9, 0xE8, 0xDE)
$Fern = [System.Drawing.Color]::FromArgb(0x3B, 0x46, 0x36)
$Night = [System.Drawing.Color]::FromArgb(0x11, 0x17, 0x10)
$GlowGreen = [System.Drawing.Color]::FromArgb(31, 0x96, 0xCE, 0x7C)
$GlowWarm = [System.Drawing.Color]::FromArgb(18, 0xE7, 0xB3, 0x4A)

# --- Helpers ----------------------------------------------------------------

# A square of the given diameter centered on a point, in supersampled units.
function New-DiscRect {
    param([double] $Diameter, [double] $CenterX, [double] $CenterY)

    $side = $Diameter * $Supersample
    [System.Drawing.RectangleF]::new(
        $CenterX * $Supersample - $side / 2.0,
        $CenterY * $Supersample - $side / 2.0,
        $side, $side)
}

# The idle mark: paper fill, caught-light sheen, fern rim. The sheen sits
# upper left, the wabi-sabi asymmetry that keeps the disc from reading as a
# mechanical dot; the rim straddles the fill edge exactly like the tray's
# Add-Rim, so the two marks stay one family.
function Add-SpiritDisc {
    param([System.Drawing.Graphics] $Graphics, [System.Drawing.RectangleF] $Bounds)

    $brush = [System.Drawing.SolidBrush]::new($Paper)
    try { $Graphics.FillEllipse($brush, $Bounds) } finally { $brush.Dispose() }

    # Sheen: a radial fade centered at 42%/38% of the disc, clipped to the
    # disc so it cannot spill past the rim.
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

# One grove ground glow: a radial wash fading to nothing, clipped by the
# canvas edges (centers sit at or beyond them, like the app's grove-ground).
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

# Renders one image: `Draw` receives a Graphics at $Supersample scale, and
# the result is downscaled to the final size and written in the pixel format
# the consumer requires (32bpp ARGB PNG, or 24bpp BMP for NSIS).
function Save-Render {
    param(
        [string] $Path,
        [int] $Width, [int] $Height,
        [System.Drawing.Imaging.PixelFormat] $PixelFormat,
        [System.Drawing.Imaging.ImageFormat] $Format,
        [scriptblock] $Draw
    )

    $large = [System.Drawing.Bitmap]::new(
        $Width * $Supersample, $Height * $Supersample,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $small = [System.Drawing.Bitmap]::new($Width, $Height, $PixelFormat)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($large)
        try {
            $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
            $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAlias
            $graphics.Clear([System.Drawing.Color]::Transparent)
            & $Draw $graphics
        } finally { $graphics.Dispose() }

        $shrink = [System.Drawing.Graphics]::FromImage($small)
        try {
            $shrink.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $shrink.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $shrink.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
            $shrink.Clear([System.Drawing.Color]::Transparent)
            $shrink.DrawImage($large, [System.Drawing.Rectangle]::new(0, 0, $Width, $Height))
        } finally { $shrink.Dispose() }

        $small.Save($Path, $Format)
        Write-Host "wrote $Path"
    } finally {
        $small.Dispose()
        $large.Dispose()
    }
}

# --- The three outputs ------------------------------------------------------

foreach ($dir in @((Split-Path $MasterPath -Parent), $NsisDir)) {
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
}

$png = [System.Drawing.Imaging.ImageFormat]::Png
$bmp = [System.Drawing.Imaging.ImageFormat]::Bmp
$argb = [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
$rgb24 = [System.Drawing.Imaging.PixelFormat]::Format24bppRgb

# The master: the idle disc alone on transparency. The OS supplies the
# ground; the fern rim is what keeps the mark present on a light one.
Save-Render -Path $MasterPath -Width $Size -Height $Size -PixelFormat $argb -Format $png -Draw {
    param($graphics)

    $bounds = New-DiscRect -Diameter ($Size * $DiscFraction) `
        -CenterX ($Size / 2.0) -CenterY ($Size / 2.0)
    Add-SpiritDisc -Graphics $graphics -Bounds $bounds
}

# Wizard header: the mark alone on night ground, centered in the strip.
Save-Render -Path (Join-Path $NsisDir 'header.bmp') `
    -Width $HeaderW -Height $HeaderH -PixelFormat $rgb24 -Format $bmp -Draw {
    param($graphics)

    $graphics.Clear($Night)
    $bounds = New-DiscRect -Diameter $HeaderDisc `
        -CenterX ($HeaderW / 2.0) -CenterY ($HeaderH / 2.0)
    Add-SpiritDisc -Graphics $graphics -Bounds $bounds
}

# Wizard sidebar: the grove at night with a stacked mark + wordmark lockup,
# the whole stack centered vertically.
Save-Render -Path (Join-Path $NsisDir 'sidebar.bmp') `
    -Width $SidebarW -Height $SidebarH -PixelFormat $rgb24 -Format $bmp -Draw {
    param($graphics)

    $graphics.Clear($Night)
    Add-Glow -Graphics $graphics -CenterX ($SidebarW * 0.08) -CenterY ($SidebarH * -0.10) `
        -Radius ($SidebarH * $GlowGreenRadius) -Color $GlowGreen
    Add-Glow -Graphics $graphics -CenterX $SidebarW -CenterY ($SidebarH * 1.10) `
        -Radius ($SidebarH * $GlowWarmRadius) -Color $GlowWarm

    $font = Resolve-WordmarkFont -EmPixels ($SidebarWordmarkEm * $Supersample)
    try {
        $format = [System.Drawing.StringFormat]::GenericTypographic
        $textSize = $graphics.MeasureString('kodabi', $font, [System.Drawing.PointF]::Empty, $format)
        $discSide = $SidebarDisc * $Supersample
        $gapSide = $SidebarGap * $Supersample
        $stackTop = ($SidebarH * $Supersample - ($discSide + $gapSide + $textSize.Height)) / 2.0

        $bounds = New-DiscRect -Diameter $SidebarDisc -CenterX ($SidebarW / 2.0) `
            -CenterY (($stackTop + $discSide / 2.0) / $Supersample)
        Add-SpiritDisc -Graphics $graphics -Bounds $bounds

        $brush = [System.Drawing.SolidBrush]::new($Paper)
        try {
            $graphics.DrawString('kodabi', $font, $brush,
                [System.Drawing.PointF]::new(
                    ($SidebarW * $Supersample - $textSize.Width) / 2.0,
                    $stackTop + $discSide + $gapSide),
                $format)
        } finally { $brush.Dispose() }
    } finally { $font.Dispose() }
}
