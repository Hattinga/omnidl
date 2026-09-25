omnidl für Linux
================

  ./omnidl                 die Desktop-App (Fenster)
  ./omnidl-cli get <Link>  Downloads im Terminal
  ./omnidl-cli serve       Web-Interface, standardmäßig http://127.0.0.1:8080

Beide Programme laufen direkt aus diesem Ordner. Beim ersten Start laden sie ihre
Werkzeuge (yt-dlp, ffmpeg, Deno, gallery-dl) nach ~/.local/share/omnidl/bin.
Ein anderer Datenordner geht mit der Umgebungsvariable OMNIDL_HOME.


Ins Anwendungsmenü aufnehmen (nur für dich, ohne root)
------------------------------------------------------

  mkdir -p ~/.local/bin ~/.local/share/applications ~/.local/share/icons/hicolor/512x512/apps
  cp omnidl omnidl-cli ~/.local/bin/
  cp omnidl.png ~/.local/share/icons/hicolor/512x512/apps/
  sed "s|^Exec=omnidl|Exec=$HOME/.local/bin/omnidl|" omnidl.desktop > ~/.local/share/applications/omnidl.desktop
  update-desktop-database ~/.local/share/applications 2>/dev/null || true

Danach steht omnidl im Menü und der Browser kann es über omnidl:// starten. Dasselbe
erledigt:

  curl -fsSL https://github.com/Hattinga/hattis-projekte/releases/latest/download/install.sh | sh -s -- --desktop

Updates: omnidl meldet neue Versionen selbst und tauscht dabei auch omnidl-cli im
selben Ordner aus. Liegen die Programme in einem Ordner ohne Schreibrecht (etwa
/usr/local/bin), bitte das Installationsskript erneut ausführen.


Entfernen
---------

  rm ~/.local/bin/omnidl ~/.local/bin/omnidl-cli \
     ~/.local/share/applications/omnidl.desktop \
     ~/.local/share/icons/hicolor/512x512/apps/omnidl.png
  rm -rf ~/.local/share/omnidl     # Werkzeuge, Einstellungen, Warteschlange


Server (Debian, Ubuntu)
-----------------------

Das Paket omnidl-cli_<version>_<amd64|arm64>.deb bringt einen systemd-Dienst mit
(eigener Benutzer "omnidl", Daten und Downloads in /var/lib/omnidl):

  sudo apt install ./omnidl-cli_<version>_amd64.deb
  sudo systemctl enable --now omnidl-web    # http://<server>:8080
  journalctl -u omnidl-web                  # zeigt das erzeugte Passwort

Eigenes Passwort, Adresse und Zielordner: /etc/default/omnidl-web

Mehr: https://github.com/Hattinga/hattis-projekte/tree/main/omnidl
