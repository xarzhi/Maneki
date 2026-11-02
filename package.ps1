# powershell -ExecutionPolicy Bypass -File .\package.ps1 0.1.0
<#
.SYNOPSIS
    Builds the Maneki installer (NSIS setup .exe) for a given version.

.DESCRIPTION
    The version lives in exactly one place - the `version` field of `[package]`
    in Cargo.toml - and everything downstream follows from it:

      * `build.rs` renders `assets/app.rc` with CARGO_PKG_VERSION before handing it
        to rc.exe, so Explorer's right-click -> Properties -> Details shows the
        version for the installed Maneki.exe;
      * `cargo packager` names the artifacts and stamps the NSIS installer with
        the same version.

    Because build.rs only re-runs when it has to, this script touches it after
    changing the version, and then does not take the outcome on trust: it reads
    the version resource back out of the built executable and fails if it does
    not match. That check is the point of the script - a setup whose exe reports
    the wrong version is worse than no setup.

    The installer bundles `vendor\libmpv-2.dll` (the actual decoder, ~120 MB).
    Without it the app installs fine and then refuses to play anything, so the
    script verifies the DLL exists before it starts.

.PARAMETER Version
    Version to package, e.g. 1.2.0. Omit it to package whatever Cargo.toml says.

.PARAMETER SkipTests
    Skip `cargo test --release`.

.EXAMPLE
    .\package.ps1 0.2.0
    .\package.ps1 0.2.0 -SkipTests
    .\package.ps1 0.2.0 -WhatIf      # only show what would change
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Position = 0)]
    [string]$Version,

    [switch]$SkipTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot -ErrorAction Stop

$manifestPath = Join-Path $PSScriptRoot 'Cargo.toml'
$exePath = Join-Path $PSScriptRoot 'target\release\maneki.exe'
$dllPath = Join-Path $PSScriptRoot 'vendor\libmpv-2.dll'
$distPath = Join-Path $PSScriptRoot 'dist'

# The package version is the first `version = "..."` inside the `[package]`
# section. It cannot be found by line pattern alone: `[dependencies.iced]` and
# friends have their own `version = "..."` at the start of a line too.
$versionPattern = '(?m)^version = "([^"]*)"'

function Get-PackageSection([string]$text) {
    $header = [regex]::Match($text, '(?m)^\[package\]\s*$')
    if (-not $header.Success) {
        throw 'Cargo.toml has no [package] section'
    }
    $start = $header.Index + $header.Length
    $next = [regex]::Match($text.Substring($start), '(?m)^\[')
    $end = if ($next.Success) { $start + $next.Index } else { $text.Length }
    return @{ Start = $start; End = $end }
}

function Get-ManifestVersion {
    $text = [System.IO.File]::ReadAllText($manifestPath)
    $section = Get-PackageSection $text
    $inside = $text.Substring($section.Start, $section.End - $section.Start)
    $found = [regex]::Matches($inside, $versionPattern)
    if ($found.Count -ne 1) {
        throw "expected exactly one version line in the [package] section, found $($found.Count)"
    }
    return $found[0].Groups[1].Value
}

function Set-ManifestVersion([string]$value) {
    $text = [System.IO.File]::ReadAllText($manifestPath)
    $section = Get-PackageSection $text
    $inside = $text.Substring($section.Start, $section.End - $section.Start)
    $updated = [regex]::Replace($inside, $versionPattern, "version = `"$value`"")
    $result = $text.Substring(0, $section.Start) + $updated + $text.Substring($section.End)
    # UTF-8 without a BOM, exactly like the file it replaces.
    [System.IO.File]::WriteAllText($manifestPath, $result, (New-Object System.Text.UTF8Encoding($false)))
}

function Assert-Success([string]$what) {
    if ($LASTEXITCODE -ne 0) {
        throw "$what failed with exit code $LASTEXITCODE"
    }
}

# Runs a native command whose stderr is not an error.
#
# Cargo writes its whole progress report to stderr, and with
# `$ErrorActionPreference = 'Stop'` PowerShell turns a native command's stderr
# into a *terminating* error: the script would die on the very run that has
# something to compile, while looking like the build failed. The exit code is the
# only thing worth trusting, so stderr is simply let through.
function Invoke-Tool {
    param([string]$What, [string[]]$Arguments)

    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & cargo @Arguments
    }
    finally {
        $ErrorActionPreference = $previous
    }
    Assert-Success $What
}

# -- 0. The decoder has to be there before anything else --------------------
if (-not (Test-Path -LiteralPath $dllPath)) {
    throw ("missing $dllPath - the installer has to contain libmpv-2.dll, " +
        "otherwise the installed player opens files and shows a black screen.")
}
$dllMB = [math]::Round((Get-Item -LiteralPath $dllPath).Length / 1MB, 1)

# -- 1. Which version are we packaging? --------------------------------------
$current = Get-ManifestVersion
if (-not $Version) {
    $Version = $current
    Write-Host "Cargo.toml already says $Version; packaging that." -ForegroundColor Cyan
}
elseif ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
    throw "'$Version' is not a version this script understands (expected e.g. 1.2.0)."
}

# -- 2. Put it in the one place everything else reads ------------------------
if ($Version -ne $current) {
    if ($PSCmdlet.ShouldProcess('Cargo.toml', "set the package version to $Version (was $current)")) {
        Set-ManifestVersion $Version
        Write-Host "Cargo.toml: $current -> $Version" -ForegroundColor Green
    }
}
else {
    Write-Host "Cargo.toml: already $Version"
}

if ($WhatIfPreference) {
    Write-Host ""
    Write-Host "WhatIf: stopping here. Without -WhatIf this would now run:" -ForegroundColor Yellow
    Write-Host "  cargo build --release"
    Write-Host "  check target\release\maneki.exe reports $Version"
    Write-Host "  cargo packager --release  (bundles libmpv-2.dll, $dllMB MB)"
    return
}

# The build script renders the resource from CARGO_PKG_VERSION, and cargo
# re-runs it when that changes - but a touch costs nothing and removes the doubt.
(Get-Item (Join-Path $PSScriptRoot 'build.rs')).LastWriteTime = Get-Date

# -- 3. A running instance holds the executable open ------------------------
# `@(...)` because a single process has no `.Count`, and StrictMode says so.
$running = @(Get-Process maneki -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Host "Stopping $($running.Count) running Maneki process(es) so the build can replace the exe." -ForegroundColor Yellow
    $running | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

# -- 4. Tests, then the build ------------------------------------------------
if (-not $SkipTests) {
    Write-Host "`n=== cargo test --release ===" -ForegroundColor Cyan
    Invoke-Tool -What 'cargo test' -Arguments @('test', '--release')
    Write-Host "tests passed"
}

Write-Host "`n=== cargo build --release ===" -ForegroundColor Cyan
Invoke-Tool -What 'cargo build' -Arguments @('build', '--release', '--bin', 'maneki')

# -- 5. The version has to be *in* the exe ----------------------------------
if (-not (Test-Path -LiteralPath $exePath)) {
    throw "no executable at $exePath after the build"
}
$info = (Get-Item -LiteralPath $exePath).VersionInfo
Write-Host "`n=== version resource of the built exe ===" -ForegroundColor Cyan
Write-Host ("  FileVersion    : {0}" -f $info.FileVersion)
Write-Host ("  ProductVersion : {0}" -f $info.ProductVersion)
Write-Host ("  ProductName    : {0}" -f $info.ProductName)
Write-Host ("  FileDescription: {0}" -f $info.FileDescription)

if ($info.FileVersion -ne $Version -or $info.ProductVersion -ne $Version) {
    throw ("the executable reports FileVersion='{0}' / ProductVersion='{1}' but {2} was asked for. " +
        "Windows' rc.exe is probably missing from PATH, so build.rs could not embed the resource - " +
        "install the Windows SDK (or run the build from a Developer Command Prompt) and try again.") -f `
        $info.FileVersion, $info.ProductVersion, $Version
}

# -- 6. Package --------------------------------------------------------------
Write-Host "`n=== cargo packager --release ===" -ForegroundColor Cyan
if (-not (Test-Path -LiteralPath $distPath)) {
    New-Item -ItemType Directory -Path $distPath | Out-Null
}
Invoke-Tool -What 'cargo packager' -Arguments @('packager', '--release')

# -- 6b. Did the "install into its own subfolder" bit make it in? ------------
# `preinstallSection` is a cargo-packager config key, and a packager upgrade can
# stop honouring it. When that happens nothing errors: the exe and uninstall.exe
# simply land in whatever folder the user picked. That is the kind of failure
# nobody notices until their Downloads folder is full of loose binaries, so read
# the NSIS script the packager just generated and stop here if it is missing.
$nsi = Get-ChildItem -LiteralPath $distPath -Filter 'installer.nsi' -Recurse -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($nsi) {
    # The template is UTF-16 with a BOM; Get-Content -Raw detects it.
    $nsiText = Get-Content -LiteralPath $nsi.FullName -Raw
    if ($nsiText -notmatch 'Section\s+DirSuffix') {
        throw ("the generated NSIS script ($($nsi.FullName)) has no DirSuffix section. " +
            "This cargo-packager version probably stopped honouring preinstallSection, so the " +
            "installer would drop the exe straight into whatever folder the user picked.")
    }
    Write-Host "installer: chosen directory gets a product-name subfolder (DirSuffix present)" -ForegroundColor Green
}
else {
    Write-Warning "no installer.nsi found under $distPath - could not verify the install-directory suffix."
}

# -- 7. Report ---------------------------------------------------------------
$setup = Get-ChildItem -LiteralPath $distPath -Filter "*$Version*-setup.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $setup) {
    $setup = Get-ChildItem -LiteralPath $distPath -Filter "*-setup.exe" |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
}
if (-not $setup) {
    throw "packaging finished but no setup executable was found in $distPath"
}

Write-Host "`n=== artifacts ===" -ForegroundColor Cyan
Get-ChildItem -LiteralPath $distPath -File |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 5 |
    ForEach-Object {
        Write-Host ("  {0,-42} {1,12:N0} bytes   {2}" -f $_.Name, $_.Length, $_.LastWriteTime)
    }

Write-Host ""
Write-Host ("installer : {0}" -f $setup.FullName) -ForegroundColor Green
Write-Host ("version   : {0} (installer reports '{1}')" -f $Version, $setup.VersionInfo.ProductVersion)
Write-Host ("exe inside: reports FileVersion '{0}'" -f $info.FileVersion)
Write-Host ("bundled   : libmpv-2.dll ({0} MB)" -f $dllMB)
Write-Host ""
Write-Host "Run the setup to install; the installed Maneki.exe reports this version in"
Write-Host "right-click -> Properties -> Details."
