# Installs, upgrades and uninstalls the MSI for the current user (no admin
# rights) and checks what the installer promises: both programs, omnidl-cli on
# the user PATH, tools and settings surviving upgrades, nothing left behind.
#
#   powershell -File packaging/test-msi.ps1 [-From omnidl-0.2.0-x64.msi]
#
# Needs target\release\omnidl.exe and omnidl-cli.exe (packaging/release.ps1)
# and WiX 5. Builds two installers from them with versions 90.0.0 and 90.0.1;
# -From installs an older MSI first to test the upgrade from it. Refuses to
# run while omnidl is installed, and puts back an omnidl:// handler that was
# registered before.

param([string]$From)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$wix = Get-Command wix -ErrorAction SilentlyContinue
$wix = if ($wix) { $wix.Source } else { Join-Path $env:USERPROFILE ".dotnet\tools\wix.exe" }
$dir = Join-Path $env:LOCALAPPDATA "Programs\omnidl"
$shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\omnidl.lnk"
$work = Join-Path $env:TEMP "omnidl-msi-test"
$failures = 0

function Check([string]$what, [bool]$ok) {
    if ($ok) { Write-Host "  ok    $what" } else { Write-Host "  FEHLT $what" -ForegroundColor Red; $script:failures++ }
}

function Msi([string]$action, [string]$msi) {
    $log = Join-Path $work ("{0}-{1}.log" -f $action.Trim("/"), [IO.Path]::GetFileNameWithoutExtension($msi))
    $p = Start-Process msiexec.exe -ArgumentList "$action `"$msi`" /qn /norestart /l*v `"$log`"" -Wait -PassThru
    if ($p.ExitCode -notin 0, 3010) { throw "msiexec ${action} ${msi}: Exit-Code $($p.ExitCode), siehe $log" }
}

function UserPath { [Environment]::GetEnvironmentVariable("Path", "User") }

# Unexpanded, with its registry type: uninstalling must restore both exactly.
function RawUserPath {
    $k = Get-Item HKCU:\Environment
    if ($null -eq $k.GetValue("Path")) { return "(kein PATH)" }
    "{0}|{1}" -f $k.GetValueKind("Path"), $k.GetValue("Path", $null, "DoNotExpandEnvironmentNames")
}

# Installed omnidl products (any version), as "product code=version".
function Installed {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    foreach ($code in $installer.RelatedProducts("{2825E7F5-088E-4A5F-B8F4-5EF130E2DE4C}")) {
        "$code=" + $installer.ProductInfo($code, "VersionString")
    }
}

# What a freshly opened terminal would find: PATH as stored in the registry.
function FreshTerminal([string]$command) {
    $psi = New-Object Diagnostics.ProcessStartInfo "cmd.exe", "/d /c $command 2>nul"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.WorkingDirectory = $env:TEMP
    $psi.EnvironmentVariables["Path"] = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + (UserPath)
    $p = [Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit()
    if ($p.ExitCode) { return $null }
    $out.Trim()
}

function PathEntries { @((UserPath) -split ";" | Where-Object { $_.TrimEnd("\") -ieq $dir }) }

if (Installed) { throw "omnidl ist installiert; erst deinstallieren, der Test würde es entfernen" }
if (Test-Path $dir) { throw "$dir gibt es schon" }
foreach ($exe in "omnidl.exe", "omnidl-cli.exe") {
    if (-not (Test-Path "target\release\$exe")) { throw "target\release\$exe fehlt; erst packaging/release.ps1" }
}

if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory $work | Out-Null
$handler = Join-Path $work "handler.reg"
$hadHandler = Test-Path "HKCU:\Software\Classes\omnidl"
if ($hadHandler) { cmd.exe /d /c "reg export HKCU\Software\Classes\omnidl `"$handler`" /y >nul 2>&1" }
$pathBefore = RawUserPath

$msis = foreach ($v in "90.0.0", "90.0.1") {
    $out = Join-Path $work "omnidl-$v-x64.msi"
    & $wix build "installer\omnidl.wxs" -arch x64 -d "Version=$v" `
        -d "Exe=target\release\omnidl.exe" -d "CliExe=target\release\omnidl-cli.exe" -o $out
    if ($LASTEXITCODE) { throw "MSI-Build fehlgeschlagen" }
    $out
}

try {
    if ($From) {
        Write-Host "Installiere $From"
        Msi "/i" (Resolve-Path $From)
        Check "ältere Version installiert" (Test-Path "$dir\omnidl.exe")
    }

    # Tools and settings the app creates itself; upgrades must keep them.
    New-Item -ItemType Directory -Force "$dir\bin\yt-dlp" | Out-Null
    Set-Content "$dir\bin\yt-dlp\yt-dlp.exe" "fake"
    Set-Content "$dir\config.toml" "parallel = 7"

    foreach ($msi in $msis) {
        Write-Host "Installiere $(Split-Path $msi -Leaf)"
        Msi "/i" $msi
        Check "omnidl.exe" (Test-Path "$dir\omnidl.exe")
        Check "omnidl-cli.exe" (Test-Path "$dir\omnidl-cli.exe")
        Check "Startmenü-Eintrag" (Test-Path $shortcut)
        $cmd = (Get-ItemProperty "HKCU:\Software\Classes\omnidl\shell\open\command")."(default)"
        Check "omnidl:// zeigt auf $dir" ($cmd -like "*$dir\omnidl.exe*")
        Check "Programmordner genau einmal im PATH des Benutzers" ((PathEntries).Count -eq 1)
        Check "PATH-Typ unverändert" (((RawUserPath) -split "\|")[0] -eq ($pathBefore -split "\|")[0])
        $where = FreshTerminal "where omnidl-cli"
        Check "neues Terminal findet omnidl-cli ($where)" ($where -like "$dir*omnidl-cli.exe")
        $version = FreshTerminal "omnidl-cli --version"
        Check "omnidl-cli --version im neuen Terminal: $version" ($version -like "omnidl-cli *")
        Check "Werkzeuge in bin\ erhalten" ((Get-Content "$dir\bin\yt-dlp\yt-dlp.exe") -eq "fake")
        Check "config.toml erhalten" ((Get-Content "$dir\config.toml") -eq "parallel = 7")
        $msiVersion = [IO.Path]::GetFileNameWithoutExtension($msi).Split("-")[1]
        $entries = @(Installed)
        Check "genau eine Version installiert ($entries)" ($entries.Count -eq 1 -and $entries[0] -like "*=$msiVersion")
    }

    Write-Host "Deinstalliere"
    Msi "/x" $msis[-1]
    Check "Programmordner entfernt (samt bin\ und config.toml)" (-not (Test-Path $dir))
    Check "Startmenü-Eintrag entfernt" (-not (Test-Path $shortcut))
    Check "PATH wie vorher (Inhalt und Typ)" ((RawUserPath) -eq $pathBefore)
    Check "omnidl-cli im neuen Terminal weg" ($null -eq (FreshTerminal "where omnidl-cli"))
    Check "HKCU\Software\omnidl entfernt" (-not (Test-Path "HKCU:\Software\omnidl"))
    Check "omnidl:// entfernt" (-not (Test-Path "HKCU:\Software\Classes\omnidl"))
    Check "nichts mehr installiert" (-not (Installed))
}
finally {
    # Whatever happened: nothing of the test stays on this machine.
    foreach ($p in @(Installed)) {
        Start-Process msiexec.exe -ArgumentList "/x $($p.Split("=")[0]) /qn /norestart" -Wait | Out-Null
    }
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
    if ($hadHandler) {
        cmd.exe /d /c "reg import `"$handler`" >nul 2>&1"
    }
    elseif (Test-Path "HKCU:\Software\Classes\omnidl") {
        Remove-Item -Recurse -Force "HKCU:\Software\Classes\omnidl"
    }
}

if ($failures) { throw "$failures Prüfung(en) fehlgeschlagen; Logs in $work" }
Write-Host "Alles in Ordnung." -ForegroundColor Green
