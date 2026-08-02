<#
.SYNOPSIS
    Install a released Toven binary on Windows.

.DESCRIPTION
    Run directly or pipe from a URL:

        # latest release, default location (%USERPROFILE%\.toven\bin)
        irm https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.ps1 | iex

        # pin an immutable version and/or choose a directory
        & ([scriptblock]::Create((irm https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.ps1))) -Version v0.1.0-alpha.2 -Dir C:\tools

    With no -Version the latest published tag (including prereleases) is resolved
    first, then its exact assets are fetched by that immutable tag. The archive
    is never trusted before its SHA-256 checksum verifies against SHA256SUMS.

.PARAMETER Version
    Release tag to install, e.g. v0.1.0-alpha.2. Defaults to the latest release.

.PARAMETER Dir
    Install directory. Defaults to %USERPROFILE%\.toven\bin.

.PARAMETER Target
    Override the Rust target triple (default x86_64-pc-windows-msvc).

.PARAMETER Repo
    Source repository (default kbukum/toven).
#>
[CmdletBinding()]
param(
    [string]$Version = $env:TOVEN_VERSION,
    [string]$Dir = $env:TOVEN_INSTALL_DIR,
    [string]$Target = $(if ($env:TOVEN_TARGET) { $env:TOVEN_TARGET } else { 'x86_64-pc-windows-msvc' }),
    [string]$Repo = $(if ($env:TOVEN_REPO) { $env:TOVEN_REPO } else { 'kbukum/toven' })
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Log($msg) { Write-Host "install: $msg" }

if (-not $Dir) { $Dir = Join-Path $env:USERPROFILE '.toven\bin' }

# Discover the newest published release tag, including prereleases.
if (-not $Version) {
    Write-Log "resolving the latest release tag for $Repo"
    $headers = @{ 'User-Agent' = 'toven-install' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)" }
    $releases = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/$Repo/releases?per_page=1"
    $Version = $releases[0].tag_name
    if (-not $Version) { throw "could not resolve the latest release tag for $Repo" }
}

$archive = "toven-$Target.zip"
$base = "https://github.com/$Repo/releases/download/$Version"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("toven-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work -Force | Out-Null

try {
    Write-Log "downloading $archive and SHA256SUMS for $Version"
    Invoke-WebRequest -Uri "$base/$archive" -OutFile (Join-Path $work $archive)
    Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS')

    Write-Log "verifying $archive against SHA256SUMS"
    $expectedLine = Get-Content (Join-Path $work 'SHA256SUMS') |
        Where-Object { $_ -match [regex]::Escape($archive) } | Select-Object -First 1
    if (-not $expectedLine) { throw "no SHA256SUMS entry for $archive" }
    $expected = ($expectedLine -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 -Path (Join-Path $work $archive)).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch for $archive (expected $expected, got $actual)" }

    Write-Log "extracting $archive"
    Expand-Archive -Path (Join-Path $work $archive) -DestinationPath $work -Force

    New-Item -ItemType Directory -Path $Dir -Force | Out-Null
    Move-Item -Force -Path (Join-Path $work 'toven.exe') -Destination (Join-Path $Dir 'toven.exe')

    $installed = & (Join-Path $Dir 'toven.exe') --version
    Write-Log "installed $installed at $(Join-Path $Dir 'toven.exe')"

    if (($env:PATH -split ';') -notcontains $Dir) {
        Write-Log "add it to PATH, e.g.:"
        Write-Log "  setx PATH `"$Dir;`$env:PATH`""
    }
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
