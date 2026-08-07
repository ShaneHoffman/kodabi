<#
.SYNOPSIS
  Signs one Windows binary with Azure Artifact Signing. Tauri's signCommand hook.

.DESCRIPTION
  `bundle.windows.signCommand` in src-tauri/tauri.bundle.conf.json points at this
  script, so the bundler runs it once per signable file: the main kodabi.exe, the
  five NSIS plugin DLLs, the bundled kodabi-mcp.exe resource, uninstall.exe (via
  the NSIS !uninstfinalize hook, from a different working directory), and finally
  the installer itself. Roughly nine invocations per release build.

  Configuration arrives entirely through the environment, and the three
  AZURE_SIGNING_* variables are the switch:

    all unset  -> print a skip line and exit 0, so a local `pnpm tauri:build`
                  produces an unsigned build with no Azure account in sight.
    partly set -> exit 1. A typo'd CI variable must fail the release loudly
                  rather than ship an unsigned installer through a green run.
    all set    -> sign, and fail the build if signing fails.

  Signing itself is signtool plus the Artifact Signing dlib
  (Azure.CodeSigning.Dlib.dll, from the Microsoft.ArtifactSigning.Client NuGet
  package), driven by a metadata file this script writes at run time. That route
  is deliberate: the alternative wrapper, artifact-signing-cli, re-runs
  `az login --service-principal` with a client *secret*, which the release
  pipeline does not have and does not want. The dlib authenticates through
  DefaultAzureCredential, which picks up the OIDC session `azure/login` already
  established, so no secret is stored anywhere.

  The RFC 3161 timestamp is not optional. Artifact Signing leaf certificates are
  valid for three days; the countersignature is the only reason an installer
  still verifies next month.

.PARAMETER FilePath
  The file to sign. Tauri substitutes it for the literal `%1` argument.

.EXAMPLE
  ./scripts/sign-windows.ps1 target/release/kodabi.exe
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath
)

$ErrorActionPreference = 'Stop'

# --- Configuration gate -------------------------------------------------------

$endpoint = $env:AZURE_SIGNING_ENDPOINT
$account = $env:AZURE_SIGNING_ACCOUNT
$certificateProfile = $env:AZURE_SIGNING_CERT_PROFILE

# GitHub renders an unset `vars.*` as the empty string, which Windows hands back
# as $null, so both spellings of "not configured" have to count as unset.
$configured = @($endpoint, $account, $certificateProfile) |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

if ($configured.Count -eq 0) {
    Write-Host "signing skipped (not configured): $FilePath"
    exit 0
}
if ($configured.Count -lt 3) {
    Write-Host 'AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_CERT_PROFILE must all be set, or all unset.' -ForegroundColor Red
    Write-Host 'Partial configuration is refused: it would otherwise ship an unsigned build through a passing run.' -ForegroundColor Red
    exit 1
}

if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) {
    Write-Host "file to sign not found: $FilePath" -ForegroundColor Red
    exit 1
}

# --- signtool -----------------------------------------------------------------

if (-not [string]::IsNullOrWhiteSpace($env:SIGNTOOL_PATH)) {
    $signtool = $env:SIGNTOOL_PATH
    if (-not (Test-Path -LiteralPath $signtool -PathType Leaf)) {
        Write-Host "SIGNTOOL_PATH is set but points at nothing: $signtool" -ForegroundColor Red
        exit 1
    }
} else {
    # Newest installed SDK wins, and no version is hard-coded: GitHub has removed
    # SDK versions from the hosted images before, which broke every pipeline that
    # spelled a path out. The dlib needs 10.0.22621.755 or later.
    $candidates = @(
        Get-ChildItem -Path 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.*\x64\signtool.exe' -ErrorAction SilentlyContinue |
            Sort-Object -Property @{ Expression = { [version]$_.Directory.Parent.Name } } -Descending
    )
    if ($candidates.Count -eq 0) {
        Write-Host 'no signtool.exe found under the Windows 10 SDK; install the Windows SDK or set SIGNTOOL_PATH' -ForegroundColor Red
        exit 1
    }
    $signtool = $candidates[0].FullName
}

# --- The Artifact Signing dlib ------------------------------------------------

$dlib = $env:AZURE_SIGNING_DLIB
if ([string]::IsNullOrWhiteSpace($dlib) -or -not (Test-Path -LiteralPath $dlib -PathType Leaf)) {
    Write-Host "AZURE_SIGNING_DLIB is unset or missing: $dlib" -ForegroundColor Red
    Write-Host 'The release workflow installs the Microsoft.ArtifactSigning.Client NuGet package and exports this path.' -ForegroundColor Red
    Write-Host 'Locally: nuget install Microsoft.ArtifactSigning.Client -x, then point it at bin\x64\Azure.CodeSigning.Dlib.dll.' -ForegroundColor Red
    exit 1
}

# --- Metadata -----------------------------------------------------------------

# Written at run time, never committed: the account and profile names carry the
# validated legal publisher identity, and this repository is public.
$metadataPath = Join-Path ([System.IO.Path]::GetTempPath()) 'kodabi-artifact-signing.json'
$metadata = [ordered]@{
    Endpoint               = $endpoint
    CodeSigningAccountName = $account
    CertificateProfileName = $certificateProfile
    # Leave only AzureCliCredential in DefaultAzureCredential's probe order. The
    # managed-identity probe in particular stalls on a hosted runner, and this
    # script runs about nine times per build.
    ExcludeCredentials     = @(
        'EnvironmentCredential',
        'WorkloadIdentityCredential',
        'ManagedIdentityCredential',
        'SharedTokenCacheCredential',
        'VisualStudioCredential',
        'VisualStudioCodeCredential',
        'AzurePowerShellCredential',
        'AzureDeveloperCliCredential',
        'InteractiveBrowserCredential'
    )
}
$metadata | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding ascii

# --- Sign ---------------------------------------------------------------------

# The bundler signs sequentially, so retrying in place is safe. The retry is for
# the timestamp authority, which is a network service on the far side of a step
# that cannot be redone once the installer is published.
for ($attempt = 1; $attempt -le 3; $attempt++) {
    & $signtool sign /v /fd SHA256 `
        /tr 'http://timestamp.acs.microsoft.com' /td SHA256 `
        /dlib $dlib /dmdf $metadataPath `
        $FilePath
    if ($LASTEXITCODE -eq 0) {
        Write-Host "signed: $FilePath"
        exit 0
    }
    Write-Host "signtool attempt $attempt of 3 failed (exit $LASTEXITCODE)" -ForegroundColor Yellow
    if ($attempt -lt 3) {
        Start-Sleep -Seconds (5 * $attempt)
    }
}

Write-Host "signing failed after 3 attempts: $FilePath" -ForegroundColor Red
exit 1
