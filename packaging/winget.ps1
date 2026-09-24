# Writes the winget manifests for a published release into packaging/winget/<version>/.
# Submit them with `wingetcreate submit packaging/winget/<version>` or as a pull
# request to https://github.com/microsoft/winget-pkgs (manifests/h/Hattinga/omnidl/<version>/).
#
#   powershell -File packaging/winget.ps1 -Version 0.2.0

param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = "Stop"

$repo = "Hattinga/omnidl"
$id = "Hattinga.omnidl"
$msi = "omnidl-$Version-x64.msi"
$url = "https://github.com/$repo/releases/download/v$Version/$msi"
$sums = (Invoke-WebRequest -UseBasicParsing "$url.sha256").Content
if ($sums -is [byte[]]) { $sums = [Text.Encoding]::ASCII.GetString($sums) }
$sha = ($sums -split '\s+')[0].ToUpper()
if ($sha.Length -ne 64) { throw "Prüfsumme für $msi nicht lesbar" }

$out = Join-Path (Split-Path $PSScriptRoot -Parent) "packaging\winget\$Version"
New-Item -ItemType Directory -Force $out | Out-Null
$schema = "1.9.0"

$files = @{
"$id.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: de-DE
ManifestType: version
ManifestVersion: $schema
"@
"$id.installer.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
InstallerType: wix
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
Protocols:
- omnidl
Commands:
- omnidl-cli
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha
ManifestType: installer
ManifestVersion: $schema
"@
"$id.locale.de-DE.yaml" = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: de-DE
Publisher: Jonas Hattinger
PublisherUrl: https://github.com/Hattinga
PackageName: omnidl
PackageUrl: https://github.com/$repo
License: MIT
LicenseUrl: https://github.com/$repo/blob/main/LICENSE
ShortDescription: Desktop-Downloader für YouTube, TikTok, Spotify und rund 1800 weitere Seiten.
Description: Link einfügen, Format wählen, fertig. Mit Warteschlange, die Neustarts übersteht, geplanten Downloads, Browser-Erweiterung und automatischen Updates.
Tags:
- downloader
- youtube
- yt-dlp
- mp3
- video
ReleaseNotesUrl: https://github.com/$repo/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: $schema
"@
}

foreach ($name in $files.Keys) {
    [IO.File]::WriteAllText((Join-Path $out $name), $files[$name].Replace("`r`n", "`n") + "`n")
}
Get-ChildItem $out | Format-Table Name -AutoSize
