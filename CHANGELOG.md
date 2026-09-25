# Änderungen

## 0.3.0

- **macOS und Linux.** Die Desktop-App läuft jetzt auch auf macOS 11+ (Apple Silicon und
  Intel, als .dmg) und auf Linux x86_64/aarch64 (.deb, .tar.gz oder Installationsskript),
  jeweils mit der Systemschrift. Die Werkzeuge kommen für jedes System passend; ein schon
  installiertes yt-dlp oder ffmpeg (Homebrew, apt) wird mitbenutzt. Einstellungen liegen
  unter `~/Library/Application Support/omnidl` bzw. `~/.local/share/omnidl`.
- **Web-Interface für Server und NAS.** `omnidl-cli serve` bringt omnidl in den Browser,
  auch aufs Handy: Links einfügen, Fortschritt live sehen, fertige Dateien herunterladen.
  Mit Passwort, im Heimnetz, hinter einem Reverse Proxy, als systemd-Dienst (Ubuntu Server,
  Debian) oder als Docker-Image `ghcr.io/hattinga/omnidl` (amd64 und arm64).
- **omnidl-cli** für das Terminal: `omnidl-cli get <Link>` lädt mit Live-Anzeige,
  `--json` liefert Ereignisse für Skripte, `omnidl-cli tools update` holt ein neues yt-dlp.
  Unter Windows legt der Installer `omnidl-cli` in den PATH; für Windows Server gibt es ein
  eigenes Zip.
- Updates holen die passende Datei fürs eigene System und tauschen `omnidl-cli` gleich mit
  aus.
- Wartende und geplante Downloads zeigen gleich ihren echten Titel statt des Links.
- Abgebrochene Werkzeug-Downloads machen dort weiter, wo sie aufgehört haben.
- Schnellerer Start.

## 0.2.0

- **Warteschlange übersteht Neustarts.** Was beim Schließen, bei einem Absturz oder
  einem Update noch nicht fertig ist, läuft beim nächsten Start weiter. Bei Playlists und
  Alben werden bereits geladene Titel übersprungen.
- **Später laden.** Uhr-Symbol neben „Laden“: in 30 Minuten, in einer Stunde, heute
  Nacht um 2:00 oder zu einer eigenen Uhrzeit. Geplante Downloads lassen sich jederzeit
  sofort starten.
- **Browser-Erweiterung** für Chrome, Edge und Firefox: Symbolleiste, Kontextmenü und
  `Alt+Umschalt+D` schicken die offene Seite oder einen Link an omnidl, als Video oder
  Audio. Läuft omnidl nicht, startet der Browser es über `omnidl://`.
- **Automatische Updates.** omnidl sucht zweimal täglich nach einer neuen Version, lädt
  sie auf Knopfdruck, prüft die SHA-256-Prüfsumme und startet neu. Laufende Downloads gehen
  danach weiter.
- **Installer.** MSI ohne Administratorrechte, mit Startmenü-Eintrag und sauberer
  Deinstallation. Bereit für winget.
- Nur noch eine Instanz: ein zweiter Start reicht seine Links an die laufende weiter.
- Fehlgeschlagene oder gestoppte Downloads lassen sich mit einem Klick erneut versuchen.
- Links lassen sich einfügen, während die Werkzeuge noch eingerichtet werden; sie warten.
- yt-dlp und ffmpeg enden jetzt zuverlässig mit der App, auch nach einem Absturz.
- Richtiges App-Symbol und Versionsangaben in der exe.
- 78 Unit-Tests statt 29.

## 0.1.0

- Erste Version: YouTube, TikTok, Spotify, Instagram, SoundCloud und die übrigen
  Seiten von yt-dlp; parallele Playlists; Oberfläche im Apple-Stil.
