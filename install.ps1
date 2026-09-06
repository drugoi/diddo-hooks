# Install diddo from GitHub Releases (Windows).
# Usage: irm https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.ps1 | iex
# Pin version: $env:DIDDO_VERSION = "0.1.0"; irm ... | iex
# Or: .\install.ps1

$ErrorActionPreference = "Stop"

$Repo = "drugoi/diddo-hooks"
$BaseUrl = "https://github.com/$Repo"
$InstallDir = if ($env:DIDDO_INSTALL_DIR) { $env:DIDDO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "diddo" }

function Get-Target {
    $arch = $env:PROCESSOR_ARCHITECTURE
    switch ($arch) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" { return "aarch64-pc-windows-msvc" }
        default {
            Write-Error "Unsupported architecture: $arch. x86_64 (AMD64) and ARM64 are supported."
        }
    }
}

function Test-Version {
    param([string]$v)
    if ($v -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$') {
        Write-Error "Invalid version '$v'"
    }
}

function Get-Version {
    if ($env:DIDDO_VERSION) {
        Test-Version $env:DIDDO_VERSION
        return $env:DIDDO_VERSION
    }
    try {
        $response = Invoke-WebRequest -Uri "$BaseUrl/releases/latest" -MaximumRedirection 0 -ErrorAction SilentlyContinue
    } catch {
        if ($_.Exception.Response.StatusCode -eq 302) {
            $location = $_.Exception.Response.Headers["Location"]
            if ($location -match "/tag/v(.+)$") {
                $resolved = $Matches[1].TrimEnd('/')
                Test-Version $resolved
                return $resolved
            }
        } else {
            throw
        }
    }
    $apiUrl = "https://api.github.com/repos/$Repo/releases/latest"
    $release = Invoke-RestMethod -Uri $apiUrl
    $tag = $release.tag_name
    if ($tag -match "^v(.+)$") {
        $resolved = $Matches[1]
    } else {
        $resolved = $tag
    }
    Test-Version $resolved
    return $resolved
}

$Target = Get-Target
$Version = Get-Version
$ZipName = "diddo-$Version-$Target.zip"
$Url = "$BaseUrl/releases/download/v$Version/$ZipName"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$TempZip = Join-Path ([System.IO.Path]::GetTempPath()) $ZipName

Write-Host "Downloading diddo $Version for $Target..."
Invoke-WebRequest -Uri $Url -OutFile $TempZip -UseBasicParsing

$SumsUrl = "$BaseUrl/releases/download/v$Version/SHA256SUMS"
$TempSums = Join-Path ([System.IO.Path]::GetTempPath()) "diddo-$Version-SHA256SUMS"
$SumsDownloaded = $true
try {
    Invoke-WebRequest -Uri $SumsUrl -OutFile $TempSums -UseBasicParsing -ErrorAction Stop
} catch {
    $SumsDownloaded = $false
}

if ($SumsDownloaded) {
    $sumsLine = Get-Content $TempSums | Where-Object { $_ -match ([regex]::Escape($ZipName) + '$') } | Select-Object -First 1
    if (-not $sumsLine) {
        Write-Error "Release v$Version has no checksum entry for $ZipName; aborting."
    }
    $expected = ($sumsLine -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 $TempZip).Hash.ToLower()
    if ($actual -ne $expected) {
        Write-Error "Checksum mismatch for ${ZipName}: expected $expected, got $actual. Aborting."
    }
    Write-Host "Checksum verified."
    Remove-Item -Path $TempSums -Force -ErrorAction SilentlyContinue
} else {
    if ($env:DIDDO_SKIP_CHECKSUM -eq "1") {
        Write-Host "WARNING: no SHA256SUMS published for v$Version; skipping verification (DIDDO_SKIP_CHECKSUM=1)."
    } else {
        Write-Error "Release v$Version does not publish SHA256SUMS (older release?). Set `$env:DIDDO_SKIP_CHECKSUM = '1' to install anyway, or pin a newer version."
    }
}

Write-Host "Extracting to $InstallDir..."
Expand-Archive -Path $TempZip -DestinationPath $InstallDir -Force
Remove-Item -Path $TempZip -Force -ErrorAction SilentlyContinue

$ExePath = Join-Path $InstallDir "diddo.exe"
if (-not (Test-Path $ExePath)) {
    Write-Error "Extraction did not produce diddo.exe in $InstallDir"
}

Write-Host "Installed diddo $Version to $ExePath"

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Host ""
    Write-Host "Add diddo to your PATH:"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', \"`$env:Path;$InstallDir\", 'User')"
    Write-Host "Then restart your terminal, or run:  & '$ExePath' --help"
}
