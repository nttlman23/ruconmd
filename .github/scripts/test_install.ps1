# Checks install.ps1 without network. Changes the user PATH and puts it back.

$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot '..\..\install.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
$dist = Join-Path $root 'dist'
$dir = Join-Path $root 'program folder'
New-Item -ItemType Directory -Path $dist | Out-Null

function Check([string]$Title, [bool]$Ok) {
    if (-not $Ok) { throw "FAILED: $Title" }
    Write-Host "ok: $Title"
}
function Refuses([string]$Title, [hashtable]$Options) {
    try { & $script @Options | Out-Null } catch { Write-Host "ok: $Title ($_)"; return }
    throw "FAILED: $Title"
}
function Raw-Path { [string](Get-Item 'HKCU:\Environment').GetValue('Path', '', 'DoNotExpandEnvironmentNames') }
function Entries { @((Raw-Path) -split ';' | Where-Object { $_ -eq $dir }).Count }

# A release made of the program given by the build, or of any small program.
function Release([string]$Version) {
    $name = "ruconmd-v$Version-windows-x86_64"
    $folder = Join-Path $dist $name
    New-Item -ItemType Directory -Path $folder | Out-Null
    Copy-Item "$env:SystemRoot\System32\whoami.exe" (Join-Path $folder 'ruconmd.exe')
    Set-Content -LiteralPath (Join-Path $folder 'LICENSE-MIT') -Value $Version
    Compress-Archive -Path $folder -DestinationPath "$folder.zip"
    $hash = (Get-FileHash -LiteralPath "$folder.zip" -Algorithm SHA256).Hash.ToLower()
    Add-Content -LiteralPath (Join-Path $dist 'SHA256SUMS') -Value "$hash  $name.zip"
}

$before = Raw-Path
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
try {
    # A reference to a variable must survive.
    $key.SetValue('Path', "%USERPROFILE%\ruconmd-test;$before", 'ExpandString')
    Release '1.0.0'
    Release '1.1.0'
    $exe = Join-Path $dir 'ruconmd.exe'
    $license = Join-Path $dir 'LICENSE-MIT'

    & $script -From $dist -Version '1.0.0' -Dir $dir
    Check 'installs' ((Test-Path -LiteralPath $exe) -and (Get-Content -LiteralPath $license) -eq '1.0.0')
    Check 'PATH is added' ((Entries) -eq 1)
    Check 'variables in PATH survive' ((Raw-Path).StartsWith('%USERPROFILE%\ruconmd-test;'))

    # Holds the program the way a running one is held: no writing, renaming is allowed.
    $held = [IO.File]::Open($exe, 'Open', 'Read', 'Read, Delete')
    try { & $script -From $dist -Version 'v1.1.0' -Dir $dir } finally { $held.Close() }
    Check 'updates a running program' ((Get-Content -LiteralPath $license) -eq '1.1.0')
    Check 'PATH is added once' ((Entries) -eq 1)

    Add-Content -LiteralPath (Join-Path $dist 'ruconmd-v1.0.0-windows-x86_64.zip') -Value 'x'
    Refuses 'damaged archive' @{ From = $dist; Version = '1.0.0'; Dir = $dir }
    Check 'the old version stays' ((Get-Content -LiteralPath $license) -eq '1.1.0')
    Refuses 'missing checksum' @{ From = $dist; Version = '2.0.0'; Dir = $dir }
    Refuses 'strange version' @{ From = $dist; Version = '..\x'; Dir = $dir }

    Set-Content -LiteralPath (Join-Path $dir 'own.txt') -Value 'user file'
    & $script -Uninstall -Dir $dir
    Check 'uninstalls' (-not (Test-Path -LiteralPath $exe) -and -not (Test-Path -LiteralPath $license))
    Check 'foreign files stay' (Test-Path -LiteralPath (Join-Path $dir 'own.txt'))
    Check 'PATH is removed' ((Entries) -eq 0)
    Check 'the rest of PATH stays' ((Raw-Path).StartsWith('%USERPROFILE%\ruconmd-test'))

    & $script -From $dist -Version '1.1.0' -Dir $dir -NoPath
    Check 'PATH is left alone' ((Entries) -eq 0)
    & $script -Uninstall -Dir $dir
} finally {
    $key.SetValue('Path', $before, 'ExpandString')
    $key.Close()
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
