# Builds everything a release needs into dist/:
#   omnidl.exe                      (what the in-app updater downloads)
#   omnidl-<version>-x64.msi        (installer, also used by winget)
#   *.sha256                        (checksums; the updater refuses files without one)
#
# Needs Rust and WiX 5:  dotnet tool install --global wix --version 5.0.2
#   powershell -File packaging/release.ps1 [-SkipTests]

param([switch]$SkipTests)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "omnidl $version"

$wix = Get-Command wix -ErrorAction SilentlyContinue
$wix = if ($wix) { $wix.Source } else { Join-Path $env:USERPROFILE ".dotnet\tools\wix.exe" }
if (-not (Test-Path $wix)) { throw "WiX fehlt: dotnet tool install --global wix --version 5.0.2" }

if (-not $SkipTests) {
    cargo test
    if ($LASTEXITCODE) { throw "Tests fehlgeschlagen" }
}
cargo build --release
if ($LASTEXITCODE) { throw "Build fehlgeschlagen" }

$dist = Join-Path $root "dist"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory $dist | Out-Null

Copy-Item "target\release\omnidl.exe" $dist
$msi = "omnidl-$version-x64.msi"
& $wix build "installer\omnidl.wxs" -arch x64 -d "Version=$version" -d "Exe=target\release\omnidl.exe" -o (Join-Path $dist $msi)
if ($LASTEXITCODE) { throw "MSI-Build fehlgeschlagen" }
Get-ChildItem $dist -Filter *.wixpdb | Remove-Item

foreach ($file in @("omnidl.exe", $msi)) {
    $hash = (Get-FileHash (Join-Path $dist $file) -Algorithm SHA256).Hash.ToLower()
    [IO.File]::WriteAllText((Join-Path $dist "$file.sha256"), "$hash  $file`n")
}

Get-ChildItem $dist | Format-Table Name, Length -AutoSize
