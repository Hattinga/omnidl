# omnidl installieren

Alle Dateien liegen unter [Releases](https://github.com/Hattinga/omnidl/releases/latest), jede
mit einer `.sha256`-Prüfsumme daneben. Es gibt zwei Programme:

- **omnidl** – die Desktop-App mit Fenster.
- **omnidl-cli** – ohne Fenster: `omnidl-cli get <Link>` lädt im Terminal,
  `omnidl-cli serve` startet das Web-Interface für Server und NAS (Standard
  `127.0.0.1:8080`, `--listen 0.0.0.0:8080` fürs Netz, Passwort mit `--password` oder
  `OMNIDL_PASSWORD`).

Beim ersten Start laden beide ihre Werkzeuge (yt-dlp, ffmpeg, Deno, gallery-dl, ~500 MB)
selbst herunter. Die Desktop-App meldet neue Versionen und aktualisiert sich auf Knopfdruck,
solange sie in einem Ordner liegt, in den der Benutzer schreiben darf; `omnidl-cli` im selben
Ordner wird dabei mit ausgetauscht.

## Windows 10/11

**Installer:** `omnidl-<version>-x64.msi` doppelklicken. Braucht keine Administratorrechte,
landet in `%LOCALAPPDATA%\Programs\omnidl`, im Startmenü und legt `omnidl-cli` in den PATH
(neues Terminal öffnen). Oder mit winget:

```powershell
winget install Hattinga.omnidl
```

**Ohne Installation:** `omnidl.exe` (und bei Bedarf `omnidl-cli.exe`) in einen beliebigen
Ordner legen und starten. Einstellungen und Werkzeuge liegen dann daneben.

## Windows Server / Server Core

`omnidl-cli-<version>-windows-x86_64.zip` nach z. B. `C:\omnidl` entpacken; Werkzeuge und
Einstellungen landen daneben:

```powershell
$v = "0.3.0"
Invoke-WebRequest "https://github.com/Hattinga/omnidl/releases/download/v$v/omnidl-cli-$v-windows-x86_64.zip" -OutFile omnidl-cli.zip
Expand-Archive omnidl-cli.zip C:\omnidl
C:\omnidl\omnidl-cli.exe serve --listen 0.0.0.0:8080 --password geheim
```

Soll das Web-Interface dauerhaft laufen, als geplante Aufgabe beim Systemstart eintragen und
Port 8080 in der Firewall freigeben.

## macOS 11 oder neuer (Apple Silicon und Intel)

`omnidl-<version>-macos.dmg` öffnen und omnidl in „Programme“ ziehen. Die App ist nicht von
Apple notarisiert; beim ersten Start meldet macOS deshalb, dass es die App nicht prüfen kann.
Dann **Systemeinstellungen → Datenschutz & Sicherheit → „Dennoch öffnen“** – oder einmal im
Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/omnidl.app
```

Terminal-Programm:

```sh
curl -fsSL https://github.com/Hattinga/omnidl/releases/latest/download/omnidl-cli-macos-universal -o omnidl-cli
chmod +x omnidl-cli && sudo mv omnidl-cli /usr/local/bin/
```

(oder `omnidl-cli-<version>-macos-universal.tar.gz` entpacken). Daten liegen unter
`~/Library/Application Support/omnidl`.

## Linux-Desktop (x86_64, aarch64)

**Debian, Ubuntu, Mint:** `omnidl_<version>_amd64.deb` (bzw. `_arm64.deb`) laden und
installieren:

```sh
sudo apt install ./omnidl_<version>_amd64.deb
```

Updates kommen dann über neue `.deb`-Dateien, nicht über die App.

**Alle Distributionen, ohne root** – installiert nach `~/.local/bin`, mit Menüeintrag und
`omnidl://` für die Browser-Erweiterung; die App aktualisiert sich dort selbst:

```sh
curl -fsSL https://github.com/Hattinga/omnidl/releases/latest/download/install.sh | sh -s -- --desktop
```

Oder `omnidl-<version>-linux-x86_64.tar.gz` entpacken und `./omnidl` starten (README.txt im
Archiv). Daten liegen unter `~/.local/share/omnidl`. Braucht glibc 2.35 oder neuer
(Ubuntu 22.04, Debian 12, Fedora 36 und alles danach).

## Ubuntu Server / Debian (Web-Interface als Dienst)

```sh
sudo apt install ./omnidl-cli_<version>_amd64.deb      # arm64: _arm64.deb
sudoedit /etc/default/omnidl-web                        # optional: OMNIDL_PASSWORD=...
sudo systemctl enable --now omnidl-web                  # → http://<server>:8080
```

Der Dienst läuft als eigener Benutzer `omnidl`; Werkzeuge, Einstellungen und Downloads liegen
in `/var/lib/omnidl` (Downloads in `/var/lib/omnidl/downloads`). Adresse, Zielordner und
Passwort stehen in `/etc/default/omnidl-web`; ohne Passwort erzeugt omnidl beim ersten Start
eines und zeigt es im Protokoll: `journalctl -u omnidl-web`.

Andere Distributionen, nur das Terminal-Programm:

```sh
curl -fsSL https://github.com/Hattinga/omnidl/releases/latest/download/install.sh | sudo sh
```

## Docker

```sh
docker run -d --name omnidl -p 8080:8080 -e OMNIDL_PASSWORD=geheim \
  -v omnidl-data:/data -v "$PWD/downloads:/downloads" ghcr.io/hattinga/omnidl:latest
```

Oder mit [`docker-compose.yml`](../docker-compose.yml): `docker compose up -d`. Ohne
`OMNIDL_PASSWORD` erzeugt omnidl ein Passwort und zeigt es in `docker logs omnidl`. Das Image
gibt es für amd64 und arm64 (Raspberry Pi 4/5, ARM-NAS). `/data` hält Werkzeuge und Einstellungen,
`/downloads` die fertigen Dateien; der Container läuft als Benutzer 10001, ein eingebundener
Download-Ordner muss für ihn beschreibbar sein (`sudo chown 10001:10001 downloads`). Selbst
bauen: `docker build -t omnidl .`
