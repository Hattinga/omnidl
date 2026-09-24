# Builds everything a Windows release needs into dist/:
#   omnidl.exe, omnidl-cli.exe                  (what the in-app updater downloads; also portable)
#   omnidl-<version>-x64.msi                    (installer with both programs, also used by winget)
#   omnidl-cli-<version>-windows-x86_64.zip     (terminal program for Windows Server / Server Core)
#   *.sha256                                    (checksums; the updater refuses files without one)
#
# Needs Rust and WiX 5:  dotnet tool install --global wix --version 5.0.2
#   powershell -File packaging/release.ps1 [-SkipTests] [-Version 0.3.0]
# -Version only changes the installer's version (for testing upgrades).

param([switch]$SkipTests, [string]$Version)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
}
Write-Host "omnidl $Version"

$wix = Get-Command wix -ErrorAction SilentlyContinue
$wix = if ($wix) { $wix.Source } else { Join-Path $env:USERPROFILE ".dotnet\tools\wix.exe" }
if (-not (Test-Path $wix)) { throw "WiX fehlt: dotnet tool install --global wix --version 5.0.2" }

if (-not $SkipTests) {
    cargo test --locked
    if ($LASTEXITCODE) { throw "Tests fehlgeschlagen" }
}
# Default features: the window (omnidl.exe) and the terminal program (omnidl-cli.exe).
cargo build --release --locked --bins
if ($LASTEXITCODE) { throw "Build fehlgeschlagen" }

$dist = Join-Path $root "dist"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory $dist | Out-Null

Copy-Item "target\release\omnidl.exe", "target\release\omnidl-cli.exe" $dist

$msi = "omnidl-$Version-x64.msi"
& $wix build "installer\omnidl.wxs" -arch x64 -d "Version=$Version" `
    -d "Exe=target\release\omnidl.exe" -d "CliExe=target\release\omnidl-cli.exe" -o (Join-Path $dist $msi)
if ($LASTEXITCODE) { throw "MSI-Build fehlgeschlagen" }
Get-ChildItem $dist -Filter *.wixpdb | Remove-Item

$zip = "omnidl-cli-$Version-windows-x86_64.zip"
Compress-Archive -Path "target\release\omnidl-cli.exe", "LICENSE" -DestinationPath (Join-Path $dist $zip)

foreach ($file in @("omnidl.exe", "omnidl-cli.exe", $msi, $zip)) {
    $hash = (Get-FileHash (Join-Path $dist $file) -Algorithm SHA256).Hash.ToLower()
    [IO.File]::WriteAllText((Join-Path $dist "$file.sha256"), "$hash  $file`n")
}

Get-ChildItem $dist | Format-Table Name, Length -AutoSize
