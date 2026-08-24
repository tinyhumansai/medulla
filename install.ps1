<#
.SYNOPSIS
    Medulla TUI installer for Windows.

.DESCRIPTION
    Downloads the prebuilt `medulla.exe` for this machine, verifies its SHA-256
    against the release manifest, and installs it to %USERPROFILE%\.medulla\bin
    (override with the MEDULLA_HOME environment variable). If the release ships
    no prebuilt binary for this platform, it falls back to building from source
    with cargo.

    The PowerShell counterpart to install.sh, which covers Linux and macOS.

.PARAMETER Version
    'latest' (default) or an explicit X.Y.Z.

.EXAMPLE
    irm https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 | iex

.EXAMPLE
    # A specific version needs the script on disk so it can take an argument:
    iwr -useb https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.ps1 -OutFile install.ps1
    .\install.ps1 -Version 0.5.3

.NOTES
    Environment:
      MEDULLA_HOME            install prefix (default: $HOME\.medulla)
      MEDULLA_UPDATE_URL      override the release manifest URL (testing)
      MEDULLA_NO_MODIFY_PATH  set to 1 to skip editing the user PATH
#>
# Write-Host is deliberate here and not a lint slip: this is an interactive
# installer, usually run as `irm ... | iex`. Its progress lines are console
# output for a human, and must not land on the success stream where they would
# become the "return value" of the piped expression.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute(
    'PSAvoidUsingWriteHost', '',
    Justification = 'Console installer: progress output is for the user, not the pipeline.')]
[CmdletBinding()]
param(
    [string] $Version = 'latest'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---- Constants ---------------------------------------------------------------

$Repo = 'tinyhumansai/medulla'
# Releases are published to $Repo above; the source lives in a separate
# repository. Only the cargo fallback below reaches for it.
$SourceRepo = if ($env:MEDULLA_SOURCE_REPO) { $env:MEDULLA_SOURCE_REPO } else { 'tinyhumansai/medulla-src' }
$DefaultManifest = "https://github.com/$Repo/releases/latest/download/latest.json"
$BinName = 'medulla.exe'

# ---- Output helpers ----------------------------------------------------------
# Everything informational goes to the information stream so `| iex` stays clean.

function Write-Info { param([string] $Message) Write-Host "==> $Message" -ForegroundColor Blue }
function Write-Ok   { param([string] $Message) Write-Host "OK  $Message" -ForegroundColor Green }
function Write-Warn { param([string] $Message) Write-Host "warning: $Message" -ForegroundColor Yellow }
function Die        { param([string] $Message) Write-Host "error: $Message" -ForegroundColor Red; exit 1 }

# ---- Platform detection ------------------------------------------------------
# Keys match the release build matrix's Rust target triples (see
# src/sdk/src/update/check.rs::platform_key). Only x86_64 ships today; arm64
# Windows runs it through the x64 emulation layer.

function Get-Target {
    $arch = $env:PROCESSOR_ARCHITECTURE
    if (-not $arch) { $arch = 'AMD64' }
    switch ($arch.ToUpperInvariant()) {
        'AMD64' { return 'x86_64-pc-windows-msvc' }
        'X86'   { Die "32-bit Windows is not supported - build from source: cargo install --path src/tui" }
        'ARM64' {
            Write-Warn 'no native arm64 Windows build; using the x86_64 binary through emulation'
            return 'x86_64-pc-windows-msvc'
        }
        default { Die "unsupported architecture '$arch'" }
    }
}

# ---- cargo source-build fallback ---------------------------------------------

function Install-FromSource {
    param([string] $Prefix)

    Write-Warn 'falling back to building from source'
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Die 'no prebuilt binary for this platform and cargo is not installed - install Rust from https://rustup.rs then re-run'
    }
    if (Test-Path 'src/tui/Cargo.toml') {
        Write-Info 'building from the local checkout (cargo install --path src/tui)'
        cargo install --path src/tui --root $Prefix --locked
    } else {
        Write-Info "building from git (cargo install --git https://github.com/$SourceRepo)"
        # Without credentials git would sit at an interactive prompt, so refuse
        # it and fail with something a reader can act on.
        $env:GIT_TERMINAL_PROMPT = '0'
        cargo install --git "https://github.com/$SourceRepo" medulla-tui --root $Prefix --locked
        if ($LASTEXITCODE -ne 0) {
            Die "could not build from source: https://github.com/$SourceRepo is not reachable with your credentials. Install a prebuilt release on a supported platform instead"
        }
    }
    if ($LASTEXITCODE -ne 0) { Die 'cargo install failed' }
}

# ---- PATH wiring -------------------------------------------------------------
# Edits the *user* PATH, never the machine one, so no elevation is needed.

function Add-ToPath {
    param([string] $Directory)

    $script:PathNeedsReload = $false

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $current) { $current = '' }

    $entries = $current -split ';' | Where-Object { $_ -ne '' }
    foreach ($entry in $entries) {
        if ($entry.TrimEnd('\') -ieq $Directory.TrimEnd('\')) { return }
    }

    if ($env:MEDULLA_NO_MODIFY_PATH -eq '1') {
        Write-Warn "MEDULLA_NO_MODIFY_PATH=1; add this to PATH yourself: $Directory"
        return
    }

    $updated = if ($current -eq '') { $Directory } else { "$current;$Directory" }
    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
    # Also update this session so `medulla` resolves without reopening a shell.
    $env:Path = "$env:Path;$Directory"
    Write-Ok "added $Directory to your user PATH"
    $script:PathNeedsReload = $true
}

# ---- Main --------------------------------------------------------------------

$prefix = if ($env:MEDULLA_HOME) { $env:MEDULLA_HOME } else { Join-Path $HOME '.medulla' }
$binDir = Join-Path $prefix 'bin'
$target = Get-Target

Write-Info "installing Medulla TUI for $target"

# Resolve the manifest URL for the requested version.
if ($env:MEDULLA_UPDATE_URL) {
    $manifestUrl = $env:MEDULLA_UPDATE_URL
} elseif ($Version -in @('latest', 'stable')) {
    $manifestUrl = $DefaultManifest
} else {
    $v = $Version.TrimStart('v')
    $manifestUrl = "https://github.com/$Repo/releases/download/v$v/latest.json"
}

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ("medulla-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

try {
    # TLS 1.2 for Windows PowerShell 5.1, whose default would refuse GitHub.
    try {
        [Net.ServicePointManager]::SecurityProtocol =
            [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch {
        # PowerShell 7 on .NET Core manages this itself and the type may be
        # absent. Not fatal: the download below reports its own failure.
        Write-Verbose "could not set TLS 1.2 explicitly: $($_.Exception.Message)"
    }

    Write-Info 'fetching release manifest'
    $manifest = $null
    try {
        $manifest = Invoke-RestMethod -Uri $manifestUrl -UseBasicParsing
    } catch {
        Write-Warn "could not fetch the release manifest ($manifestUrl)"
    }

    $entry = $null
    if ($manifest) {
        if ($manifest.PSObject.Properties.Name -contains 'version' -and $manifest.version) {
            Write-Info "latest release: v$($manifest.version)"
        }
        if ($manifest.PSObject.Properties.Name -contains 'platforms' -and
            $manifest.platforms.PSObject.Properties.Name -contains $target) {
            $entry = $manifest.platforms.$target
        } else {
            Write-Warn "the release ships no prebuilt binary for $target"
        }
    }

    if (-not $entry) {
        New-Item -ItemType Directory -Path $prefix -Force | Out-Null
        Install-FromSource -Prefix $prefix
    } else {
        $assetUrl = $entry.url
        $assetSha = $entry.sha256
        $archive = Join-Path $workDir 'asset.zip'

        Write-Info "downloading $(Split-Path $assetUrl -Leaf)"
        try {
            Invoke-WebRequest -Uri $assetUrl -OutFile $archive -UseBasicParsing
        } catch {
            Die "download failed: $assetUrl"
        }

        if ($assetSha) {
            $got = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($got -ne $assetSha.ToLowerInvariant()) {
                Die "checksum mismatch: expected $assetSha, got $got"
            }
            Write-Ok 'checksum verified'
        }

        Write-Info 'extracting'
        $extract = Join-Path $workDir 'extract'
        New-Item -ItemType Directory -Path $extract -Force | Out-Null
        Expand-Archive -Path $archive -DestinationPath $extract -Force

        # The archive nests the binary under medulla-<tag>-<target>\, so search
        # the tree rather than assuming the layout.
        $binary = Get-ChildItem -Path $extract -Filter $BinName -Recurse -File |
            Select-Object -First 1
        if (-not $binary) { Die "no '$BinName' found in the downloaded archive" }

        New-Item -ItemType Directory -Path $binDir -Force | Out-Null
        $dest = Join-Path $binDir $BinName
        # A running medulla.exe holds a lock on its own image; move it aside the
        # way `medulla update` does rather than failing the install.
        if (Test-Path $dest) {
            try {
                Move-Item -Path $dest -Destination "$dest.old" -Force
            } catch {
                Die "could not replace $dest - close any running medulla and re-run"
            }
        }
        Copy-Item -Path $binary.FullName -Destination $dest -Force
        Write-Ok "installed $BinName to $dest"
    }

    Add-ToPath -Directory $binDir

    # Best-effort banner detail only; a binary that cannot run still installed,
    # and the user's own `medulla version` will report the real reason.
    $installed = ''
    try {
        $installed = & (Join-Path $binDir $BinName) version 2>$null | Select-Object -First 1
    } catch {
        Write-Verbose "could not run the installed binary: $($_.Exception.Message)"
    }

    Write-Host ''
    Write-Ok 'Medulla TUI is installed.'
    if ($installed) { Write-Info $installed }
    Write-Host ''
    Write-Host 'Next steps:'
    if ($script:PathNeedsReload) {
        Write-Host '  1. Open a new terminal (or use the full path below)'
        Write-Host '  2. Log in:         medulla login'
        Write-Host '  3. Launch the TUI: medulla'
    } else {
        Write-Host '  1. Log in:         medulla login'
        Write-Host '  2. Launch the TUI: medulla'
    }
    Write-Host ''
    Write-Info 'Without credentials, medulla opens a login screen; press m to look around offline.'
    Write-Info 'Update anytime with medulla update.'

    # Explicit, or the exit code is whatever the cleanup below happened to leave
    # behind. Windows PowerShell 5.1 maps a trailing failed statement to exit 1,
    # and `powershell -command ". 'install.ps1'"` then reports a completed
    # install as a failure. `exit` still runs the `finally` block.
    exit 0
} finally {
    # Best-effort: a leftover temp directory is not worth failing an install
    # that already succeeded, and must not colour the exit code.
    if (Test-Path $workDir) {
        try {
            Remove-Item -Recurse -Force $workDir -ErrorAction Stop
        } catch {
            Write-Verbose "could not remove $($workDir): $($_.Exception.Message)"
        }
    }
}
