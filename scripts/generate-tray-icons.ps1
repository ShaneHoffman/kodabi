<#
.SYNOPSIS
    Draws the three 32x32 SpiritMark tray icons Kodabi's tray swaps between.

.DESCRIPTION
    Companion to docs/SPIRIT_MARK.md: the tray icon is the only capture
    surface once the window is hidden, so it has to answer "am I being
    recorded?" at a glance. The mark carries that in three static states,
    mirroring src/captureLabel.ts::markMode and the Rust `TrayIconKind`
    in src-tauri/src/capture_control.rs:

      tray-idle       solid ink disc      no capture is running
      tray-engaged    hollow ink ring     starting, or reconnecting: a session
                                          is engaged but nothing reaches disk,
                                          so it wears no green
      tray-recording  green disc + aura   audio is genuinely being recorded

    The green is reserved for that last state and spent nowhere else, which
    is what makes its absence trustworthy (SPIRIT_MARK.md "Trust / consent
    signal").

    Drawing notes. Each mark is rendered at 8x (256px) with antialiasing and
    downscaled bicubic, because plain GDI+ antialiasing at 32px leaves the
    disc edges ragged. Output is 32bpp ARGB PNG, which is what Tauri's
    `include_image!` requires (it rejects RGB24 and palette PNGs at compile
    time).

    Colors are the design tokens from design/tokens.css, hard-coded here
    because a PNG cannot read a CSS custom property. Keep them in sync:
      #E9E8DE  --k-paper        the ink treatment, inverted
      #3B4636  --k-fern         rim, so the marks also read on a light taskbar
      #86A67E  --k-green-night  the living green (light sibling: --k-green
                                #5F7E5A; the night value is brighter and the
                                Windows 11 taskbar is dark by default)
      rgba(255,255,255,.16)     --sheen

.NOTES
    `include_image!` embeds these pixels at compile time and cargo does not
    track the PNGs as dependencies of capture_control.rs. After regenerating,
    touch that file (or `cargo clean -p kodabi`) or the build will keep the
    stale embedding.

.EXAMPLE
    .\scripts\generate-tray-icons.ps1
#>

[CmdletBinding()]
param(
    # Where the PNGs land. Defaults to the tray icon dir include_image! reads.
    [string] $OutDir = (Join-Path $PSScriptRoot '..\src-tauri\icons\tray')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

# --- Tunables -------------------------------------------------------------
# Geometry is expressed in final 32px units and scaled up for drawing, so
# these read against the icon you actually see in the tray.
$Size = 32          # committed icon size; Windows scales it 16px..32px by DPI
$Supersample = 8    # draw at 8x, downscale bicubic
$InkDiameter = 19.0 # idle disc / engaged ring outer diameter
$RingStroke = 3.4   # engaged ring wall thickness
$CoreDiameter = 15.0 # recording core disc (smaller: the aura carries the rest)
$AuraDiameter = 30.0 # aura reach; stays inside 32 so it is never cropped
# The pale fill is close in value to a light taskbar, so the rim is what
# gives the mark presence there. Thin enough to stay a rim at 16px, thick
# enough to survive the downscale.
$RimStroke = 1.9
$AuraAlpha = 150    # aura center opacity, fading to 0 at its edge
$SheenAlpha = 41    # --sheen: 0.16 * 255

$Paper = [System.Drawing.Color]::FromArgb(0xE9, 0xE8, 0xDE)
$Fern = [System.Drawing.Color]::FromArgb(0x3B, 0x46, 0x36)
$Green = [System.Drawing.Color]::FromArgb(0x86, 0xA6, 0x7E)

# --- Helpers --------------------------------------------------------------

# A square centered on the canvas, in supersampled device units.
function New-CenteredRect {
    param([double] $Diameter, [int] $Canvas)

    $side = $Diameter * $Supersample
    $origin = ($Canvas - $side) / 2.0
    [System.Drawing.RectangleF]::new($origin, $origin, $side, $side)
}

# The rim every pale mark wears, so it survives a light taskbar too.
function Add-Rim {
    param([System.Drawing.Graphics] $Graphics, [System.Drawing.RectangleF] $Bounds)

    $pen = [System.Drawing.Pen]::new($Fern, [float]($RimStroke * $Supersample))
    try { $Graphics.DrawEllipse($pen, $Bounds) } finally { $pen.Dispose() }
}

# Renders one mark: `Draw` receives a Graphics at $Supersample scale, and the
# result is downscaled to $Size and written as 32bpp ARGB PNG.
function Save-Mark {
    param([string] $Name, [scriptblock] $Draw)

    $canvas = $Size * $Supersample
    $large = [System.Drawing.Bitmap]::new($canvas, $canvas, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $small = [System.Drawing.Bitmap]::new($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($large)
        try {
            $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.Clear([System.Drawing.Color]::Transparent)
            & $Draw $graphics $canvas
        } finally { $graphics.Dispose() }

        $shrink = [System.Drawing.Graphics]::FromImage($small)
        try {
            $shrink.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $shrink.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $shrink.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
            $shrink.Clear([System.Drawing.Color]::Transparent)
            $shrink.DrawImage($large, [System.Drawing.Rectangle]::new(0, 0, $Size, $Size))
        } finally { $shrink.Dispose() }

        $path = Join-Path $OutDir "$Name.png"
        $small.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        Write-Host "wrote $path"
    } finally {
        $small.Dispose()
        $large.Dispose()
    }
}

# --- The three marks ------------------------------------------------------

if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}

# Idle: the resting logo. Present, plainly not listening.
Save-Mark 'tray-idle' {
    param($graphics, $canvas)

    $bounds = New-CenteredRect -Diameter $InkDiameter -Canvas $canvas
    $brush = [System.Drawing.SolidBrush]::new($Paper)
    try { $graphics.FillEllipse($brush, $bounds) } finally { $brush.Dispose() }
    Add-Rim -Graphics $graphics -Bounds $bounds
}

# Engaged: a session is running but nothing is recorded. Hollow, so it is
# visibly neither the idle mark nor on air. The static stand-in for the
# in-window mark's ink pulse.
Save-Mark 'tray-engaged' {
    param($graphics, $canvas)

    $outer = New-CenteredRect -Diameter $InkDiameter -Canvas $canvas
    # Stroke straddles the path, so inset by half of it to keep the outer
    # edge on the same diameter as the idle disc.
    $inset = [float]($RingStroke * $Supersample / 2.0)
    $bounds = [System.Drawing.RectangleF]::new(
        $outer.X + $inset, $outer.Y + $inset,
        $outer.Width - 2 * $inset, $outer.Height - 2 * $inset)

    $pen = [System.Drawing.Pen]::new($Paper, [float]($RingStroke * $Supersample))
    try { $graphics.DrawEllipse($pen, $bounds) } finally { $pen.Dispose() }
    Add-Rim -Graphics $graphics -Bounds $outer
}

# Recording: the one green, with its aura baked in. A tray icon cannot
# breathe, so the aura carries the "attending to you" presence statically.
Save-Mark 'tray-recording' {
    param($graphics, $canvas)

    $aura = New-CenteredRect -Diameter $AuraDiameter -Canvas $canvas
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    try {
        $path.AddEllipse($aura)
        $gradient = [System.Drawing.Drawing2D.PathGradientBrush]::new($path)
        try {
            $gradient.CenterColor = [System.Drawing.Color]::FromArgb($AuraAlpha, $Green)
            $gradient.SurroundColors = @([System.Drawing.Color]::FromArgb(0, $Green))
            $graphics.FillEllipse($gradient, $aura)
        } finally { $gradient.Dispose() }
    } finally { $path.Dispose() }

    $core = New-CenteredRect -Diameter $CoreDiameter -Canvas $canvas
    $brush = [System.Drawing.SolidBrush]::new($Green)
    try { $graphics.FillEllipse($brush, $core) } finally { $brush.Dispose() }
    Add-Rim -Graphics $graphics -Bounds $core

    # Caught light, upper left: the wabi-sabi asymmetry that keeps the core
    # from reading as a mechanical status LED.
    $sheen = [System.Drawing.RectangleF]::new(
        $core.X + $core.Width * 0.20, $core.Y + $core.Height * 0.14,
        $core.Width * 0.42, $core.Height * 0.30)
    $sheenBrush = [System.Drawing.SolidBrush]::new(
        [System.Drawing.Color]::FromArgb($SheenAlpha, 255, 255, 255))
    try { $graphics.FillEllipse($sheenBrush, $sheen) } finally { $sheenBrush.Dispose() }
}
