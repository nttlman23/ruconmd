# Installs ruconmd for the current user, no administrator rights needed.
#
#   install.ps1 [-Version X.Y.Z] [-Dir PATH] [-From PATH] [-NoPath]
#   install.ps1 -Uninstall [-Dir PATH]
#
# -From takes the archive and SHA256SUMS from a folder, without network.
# This file is ASCII only: Windows PowerShell 5.1 reads it in the system code page.

[CmdletBinding()]
param(
    [string]$Version = '',
    [string]$Dir = '',
    [string]$From = '',
    [switch]$NoPath,
    [switch]$Uninstall
)

function Set-UserPath([string]$Folder, [bool]$Present) {
    # The registry is used directly: the .NET call would expand %VARIABLES%.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $parts = @($old -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ne $Folder.TrimEnd('\') })
        if ($Present) { $parts += $Folder }
        $new = $parts -join ';'
        if ($new -ne $old) {
            $key.SetValue('Path', $new, 'ExpandString')
            # Setting any variable tells running programs about the change.
            [Environment]::SetEnvironmentVariable('RUCONMD_PATH_CHANGED', $null, 'User')
        }
    } finally { $key.Close() }
    $now = @($env:Path -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ne $Folder.TrimEnd('\') })
    if ($Present) { $now += $Folder }
    $env:Path = $now -join ';'
}

function Get-Latest([string]$Repo) {
    $list = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'ruconmd-install' } `
        -Uri "https://api.github.com/repos/$Repo/releases?per_page=30"
    $ready = @($list | Where-Object { -not $_.draft })
    $stable = @($ready | Where-Object { -not $_.prerelease })
    if ($stable.Count -gt 0) { return $stable[0].tag_name.Substring(1) }
    if ($ready.Count -gt 0) { return $ready[0].tag_name.Substring(1) }
    throw 'no releases'
}

# A running program cannot be overwritten but can be renamed.
function Move-Aside([string]$File) {
    if (-not (Test-Path -LiteralPath $File)) { return }
    foreach ($n in 0..15) {
        $old = "$File.old$n"
        if (Test-Path -LiteralPath $old) {
            Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
            if (Test-Path -LiteralPath $old) { continue }
        }
        Move-Item -LiteralPath $File -Destination $old
        return $old
    }
    throw 'previous versions are still running'
}

function Install-Ruconmd {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'nttlman23/ruconmd'
    $files = 'ruconmd.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD-PARTY.txt', 'README.md', 'CHANGELOG.md', 'config.example.toml'

    if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA 'Programs\ruconmd' }
    $Dir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Dir)

    if ($Uninstall) {
        # Only own files: the folder may hold something else.
        foreach ($file in $files) {
            $path = Join-Path $Dir $file
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
        }
        Get-ChildItem -LiteralPath $Dir -Filter 'ruconmd.exe.old*' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
        if ((Test-Path -LiteralPath $Dir) -and -not (Get-ChildItem -LiteralPath $Dir -Force)) { Remove-Item -LiteralPath $Dir }
        Set-UserPath $Dir $false
        Write-Host "Removed $Dir. Settings and recovery copies are kept."
        return
    }

    if (-not [Environment]::Is64BitOperatingSystem) { throw 'only 64-bit Windows is supported' }
    # Windows PowerShell 5.1 may not offer TLS 1.2 by itself.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    if (-not $Version) {
        if ($From) { throw '-From needs -Version' }
        $Version = Get-Latest $repo
    }
    $Version = $Version -replace '^v', ''
    if ($Version -notmatch '^[0-9A-Za-z.-]+$') { throw "bad version '$Version'" }

    $name = "ruconmd-v$Version-windows-x86_64"
    $temp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $temp | Out-Null
    try {
        $archive = Join-Path $temp "$name.zip"
        $sums = Join-Path $temp 'SHA256SUMS'
        if ($From) {
            Copy-Item -LiteralPath (Join-Path $From "$name.zip") -Destination $archive
            Copy-Item -LiteralPath (Join-Path $From 'SHA256SUMS') -Destination $sums
        } else {
            $base = "https://github.com/$repo/releases/download/v$Version"
            Write-Host "Downloading ruconmd $Version"
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$name.zip" -OutFile $archive
            Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums
        }

        $lines = @(Get-Content -LiteralPath $sums | Where-Object { $_ -match ('^[0-9a-fA-F]{64} [ *]' + [regex]::Escape("$name.zip") + '$') })
        if ($lines.Count -ne 1) { throw "no checksum for $name.zip" }
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        if ($actual -ne $lines[0].Substring(0, 64)) { throw 'checksum mismatch, nothing is installed' }

        Expand-Archive -LiteralPath $archive -DestinationPath $temp
        $source = Join-Path $temp $name
        if (-not (Test-Path -LiteralPath (Join-Path $source 'ruconmd.exe'))) { throw 'no program in the archive' }

        New-Item -ItemType Directory -Path $Dir -Force | Out-Null
        $program = Join-Path $Dir 'ruconmd.exe'
        $old = Move-Aside $program
        try {
            Copy-Item -LiteralPath (Join-Path $source 'ruconmd.exe') -Destination $program
        } catch {
            if ($old) { Move-Item -LiteralPath $old -Destination $program -Force }
            throw
        }
        if ($old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
        foreach ($file in $files) {
            $path = Join-Path $source $file
            if ($file -ne 'ruconmd.exe' -and (Test-Path -LiteralPath $path)) { Copy-Item -LiteralPath $path -Destination $Dir -Force }
        }
    } finally {
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
    }

    if (-not $NoPath) { Set-UserPath $Dir $true }
    Write-Host "Installed $program ($(& $program --version))"
    if (-not $NoPath) { Write-Host 'Open a new terminal window to use it by name.' }
}

# The call is the last line: a partly downloaded script does nothing.
Install-Ruconmd
