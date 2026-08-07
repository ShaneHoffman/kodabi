<#
.SYNOPSIS
  Publishes the model files the app downloads on first run to a GitHub release.

.DESCRIPTION
  The manifest compiled into the app (crates/kodabi-core/src/models/manifest.json)
  is the source of truth: it names each file, its size, its SHA-256, and the flat
  release-asset name to publish it under. This script reads that manifest, checks
  every local file against it, and only then creates the release and uploads.

  Verification comes first and covers everything, deliberately. A mismatched
  upload is not a recoverable mistake: every installed app verifies what it
  downloads against the same digests, so a wrong byte means every install fails
  at the last step, and re-uploading an asset does not fix the copies already
  cached by GitHub's CDN.

  Assets are uploaded with --clobber so a resumed or corrected run replaces its
  own partial work rather than erroring.

.PARAMETER ModelsDir
  Directory holding the reference copies. Expected layout, matching the manifest:
    <ModelsDir>/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/{encoder,decoder,joiner}.int8.onnx
    <ModelsDir>/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/tokens.txt
    <ModelsDir>/silero_vad.onnx
    <ModelsDir>/bge-small-en-v1.5/{model.onnx,tokenizer.json,config.json,special_tokens_map.json,tokenizer_config.json}

.PARAMETER WhatIf
  Verify every file and report, but upload nothing.

.EXAMPLE
  ./scripts/upload-models.ps1 -ModelsDir C:\Users\me\kodabi-models -WhatIf
  ./scripts/upload-models.ps1 -ModelsDir C:\Users\me\kodabi-models
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory = $true)]
    [string]$ModelsDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot 'crates/kodabi-core/src/models/manifest.json'
if (-not (Test-Path $manifestPath)) {
    throw "manifest not found at $manifestPath"
}
$manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json

# Where each set's files sit locally. The manifest's `dir` is the *installed*
# layout; the reference copies keep the upstream archive's own folder names, so
# the one place they differ is mapped here.
$localDirs = @{
    'parakeet-tdt-0.6b-v2-int8' = 'sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8'
    'silero-vad'                = ''
    'bge-small-en-v1.5'         = 'bge-small-en-v1.5'
}

Write-Host "Manifest: schema $($manifest.schema_version), release $($manifest.release_tag)"
Write-Host "Verifying against $ModelsDir`n"

$uploads = [System.Collections.Generic.List[object]]::new()
$failures = [System.Collections.Generic.List[string]]::new()

foreach ($set in $manifest.model_sets) {
    if (-not $localDirs.ContainsKey($set.id)) {
        $failures.Add("no local directory mapped for model set '$($set.id)'")
        continue
    }
    $setDir = $localDirs[$set.id]
    foreach ($file in $set.files) {
        $path = if ([string]::IsNullOrEmpty($setDir)) {
            Join-Path $ModelsDir $file.name
        } else {
            Join-Path (Join-Path $ModelsDir $setDir) $file.name
        }

        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            $failures.Add("$($file.asset): missing at $path")
            continue
        }

        $actualSize = (Get-Item -LiteralPath $path).Length
        if ($actualSize -ne $file.size) {
            $failures.Add("$($file.asset): size $actualSize, manifest says $($file.size)")
            continue
        }

        $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $file.sha256.ToLowerInvariant()) {
            $failures.Add("$($file.asset): sha256 $actualHash, manifest says $($file.sha256)")
            continue
        }

        Write-Host "  ok  $($file.asset)  ($([math]::Round($actualSize / 1MB)) MB)"
        $uploads.Add([pscustomobject]@{ Path = $path; Asset = $file.asset })
    }
}

if ($failures.Count -gt 0) {
    Write-Host "`nRefusing to upload. Every file must match the manifest exactly:" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    throw "$($failures.Count) file(s) did not match the manifest"
}

$totalBytes = ($uploads | ForEach-Object { (Get-Item -LiteralPath $_.Path).Length } | Measure-Object -Sum).Sum
Write-Host "`nAll $($uploads.Count) files match. Total $([math]::Round($totalBytes / 1MB)) MB."

if (-not $PSCmdlet.ShouldProcess($manifest.release_tag, "create release and upload $($uploads.Count) assets")) {
    Write-Host 'Nothing uploaded (-WhatIf).'
    return
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw 'the GitHub CLI (gh) is required to publish the release'
}

# Uploads are additive, so an existing release is reused rather than replaced:
# recreating it would break every installed app pinned to these URLs.
$existing = gh release view $manifest.release_tag 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host "Creating release $($manifest.release_tag)..."
    $notes = @"
Machine-learning models Kodabi downloads on first run.

These are third-party works redistributed under their own licences, not covered
by Kodabi's AGPL-3.0 licence. See NOTICE.txt, written beside the models on
install, and the ``license`` block of each set in
``crates/kodabi-core/src/models/manifest.json``.

Referenced by manifest schema $($manifest.schema_version). Assets here are immutable:
a model change ships as a new ``models-v?`` release and a manifest edit together.
"@
    # --prerelease is load-bearing, not a status claim. The auto-updater's
    # endpoint is `releases/latest/download/latest.json`, and a models release
    # taking that slot would 404 the manifest for every installed app, silently
    # on both sides: the app just quietly stops finding updates.
    #
    # It has to be --prerelease rather than --latest=false, which is what this
    # first reached for and is NOT enough: `/releases/latest` falls back to the
    # most recent non-draft, non-prerelease release whenever nothing is
    # explicitly marked latest, so with one release in the repo the flag bought
    # nothing. GitHub excludes prereleases from that computation outright, which
    # holds no matter what gets published later. Assets stay downloadable by tag
    # either way; only the "latest" pointer changes.
    gh release create $manifest.release_tag --title "Models $($manifest.release_tag)" --notes $notes --prerelease
    if ($LASTEXITCODE -ne 0) { throw 'gh release create failed' }
} else {
    Write-Host "Release $($manifest.release_tag) already exists; uploading into it."
}

# The asset name must be exactly what the manifest expects, and a local
# filename rarely is: the sets contain a `model.onnx`, a `config.json` and a
# `tokenizer.json` whose names say nothing about which model they belong to, so
# the manifest flattens each to `<set-id>.<file>`.
#
# This staging directory is how the rename happens. The obvious spelling,
# gh's `path#name` form, does NOT do it — `#` sets an asset's display *label*
# and leaves the name as the file's own basename. That silently published ten
# assets under the wrong names once, and nothing caught it until a download
# 404'd, because every local check had already passed. gh takes the name from
# the file, so the file is what has to carry it.
#
# Hardlinks, not copies: this is 759 MB, and a link costs nothing when the temp
# directory shares a volume with the models. It usually does; when it doesn't,
# fall back to copying rather than failing.
$staging = Join-Path ([System.IO.Path]::GetTempPath()) "kodabi-models-upload-$PID"
New-Item -ItemType Directory -Path $staging -Force | Out-Null
try {
    foreach ($upload in $uploads) {
        $staged = Join-Path $staging $upload.Asset
        try {
            New-Item -ItemType HardLink -Path $staged -Value $upload.Path -ErrorAction Stop | Out-Null
        } catch {
            Copy-Item -LiteralPath $upload.Path -Destination $staged
        }
    }

    foreach ($upload in $uploads) {
        Write-Host "Uploading $($upload.Asset)..."
        gh release upload $manifest.release_tag (Join-Path $staging $upload.Asset) --clobber
        if ($LASTEXITCODE -ne 0) { throw "failed to upload $($upload.Asset)" }
    }
} finally {
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
}

# What the app will actually request, checked against what is actually there.
# Every local verification above passed while the release was wrong, so the
# only check that means anything is this one, made against the published names.
Write-Host "`nVerifying published asset names..."
$published = gh release view $manifest.release_tag --json assets --jq '.assets[].name'
if ($LASTEXITCODE -ne 0) { throw 'could not read back the release assets' }
$publishedSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$published)
$missing = $uploads | Where-Object { -not $publishedSet.Contains($_.Asset) }
if ($missing) {
    $missing | ForEach-Object { Write-Host "  MISSING  $($_.Asset)" -ForegroundColor Red }
    throw "$($missing.Count) asset(s) are not published under the name the app requests"
}
Write-Host "All $($uploads.Count) assets carry the names the manifest expects."

Write-Host "`nDone. $($uploads.Count) assets published to $($manifest.release_tag)."
Write-Host 'Verify a fresh install downloads and passes verification before announcing the build.'
