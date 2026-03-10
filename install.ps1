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

function Get-Version {
    if ($env:DIDDO_VERSION) {
        return $env:DIDDO_VERSION
    }
    try {
        $response = Invoke-WebRequest -Uri "$BaseUrl/releases/latest" -MaximumRedirection 0 -ErrorAction SilentlyContinue
    } catch {
        if ($_.Exception.Response.StatusCode -eq 302) {
            $location = $_.Exception.Response.Headers["Location"]
            if ($location -match "/tag/v(.+)$") {
                return $Matches[1].TrimEnd('/')
            }
        }
    }
    $apiUrl = "https://api.github.com/repos/$Repo/releases/latest"
    $release = Invoke-RestMethod -Uri $apiUrl
    $tag = $release.tag_name
    if ($tag -match "^v(.+)$") {
        return $Matches[1]
    }
    return $tag
}

$Target = Get-Target
$Version = Get-Version
$ZipName = "diddo-$Version-$Target.zip"
$Url = "$BaseUrl/releases/download/v$Version/$ZipName"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$TempZip = Join-Path ([System.IO.Path]::GetTempPath()) $ZipName

Write-Host "Downloading diddo $Version for $Target..."
Invoke-WebRequest -Uri $Url -OutFile $TempZip -UseBasicParsing

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
