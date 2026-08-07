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
    # --latest=false is load-bearing, not tidiness. The auto-updater's endpoint
    # is `releases/latest/download/latest.json`, and GitHub hands "latest" to
    # whichever release is newest by default. A models release taking that slot
    # would 404 the manifest for every installed app until the next app
    # release, and the failure is silent on both sides: the app just quietly
    # stops finding updates. Model releases are a side channel and must never
    # claim to be the app's current version.
    gh release create $manifest.release_tag --title "Models $($manifest.release_tag)" --notes $notes --latest=false
    if ($LASTEXITCODE -ne 0) { throw 'gh release create failed' }
} else {
    Write-Host "Release $($manifest.release_tag) already exists; uploading into it."
}

foreach ($upload in $uploads) {
    Write-Host "Uploading $($upload.Asset)..."
    # The asset name must be what the manifest expects, and a local filename
    # rarely is (three sets contain a differently-scoped model.onnx), so every
    # upload is renamed with gh's `path#name` form.
    gh release upload $manifest.release_tag "$($upload.Path)#$($upload.Asset)" --clobber
    if ($LASTEXITCODE -ne 0) { throw "failed to upload $($upload.Asset)" }
}

Write-Host "`nDone. $($uploads.Count) assets published to $($manifest.release_tag)."
Write-Host 'Verify a fresh install downloads and passes verification before announcing the build.'
