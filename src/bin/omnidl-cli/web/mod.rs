//! `omnidl-cli serve`: the web interface for servers, NAS boxes and the home
//! network. The engine runs as in the desktop app; any browser on the network
//! adds links, watches progress and fetches finished files.

mod api;
mod auth;
mod files;
mod hub;
mod net;

use anyhow::{Context, Result, anyhow};
use omnidl::config::{self, Config};
use omnidl::engine::{self, Sink};
use omnidl::launch::Launch;
use omnidl::sys;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(clap::Args)]
pub struct Args {
    /// Adresse und Port. 0.0.0.0:8080 macht omnidl im ganzen Netz erreichbar
    /// (dann immer mit Passwort).
    #[arg(long, value_name = "ADRESSE:PORT", default_value = "127.0.0.1:8080", value_parser = net::parse_listen)]
    pub listen: SocketAddr,
    /// Passwort für die Anmeldung; auch über die Umgebungsvariable OMNIDL_PASSWORD.
    /// Ohne wird im Netz eines erzeugt und in web.toml gespeichert.
    #[arg(long, value_name = "PASSWORT")]
    pub password: Option<String>,
    /// Ordner für Downloads; ersetzt die Einstellung, solange der Server läuft.
    #[arg(long, value_name = "ORDNER")]
    pub dir: Option<PathBuf>,
    /// Anmelde-Cookie nur über HTTPS senden (hinter einem HTTPS-Reverse-Proxy).
    #[arg(long)]
    pub secure_cookie: bool,
}

pub fn run(args: Args) -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("omnidl-cli serve: Laufzeitumgebung nicht startbar: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(serve(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omnidl-cli serve: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(args: Args) -> Result<()> {
    let base = config::base_dir();
    let mut cfg = Config::load();
    let stored_dir = cfg.download_dir.clone();
    if let Some(dir) = &args.dir {
        cfg.download_dir = std::path::absolute(dir).context("Download-Ordner ungültig")?;
    }
    if let Err(e) = std::fs::create_dir_all(&cfg.download_dir) {
        eprintln!("Warnung: {} lässt sich nicht anlegen: {e}", cfg.download_dir.display());
    }

    let loopback = net::is_loopback(&args.listen);
    let given = args.password.or_else(|| std::env::var("OMNIDL_PASSWORD").ok());
    let web_file = base.join("web.toml");
    let password = auth::resolve(given, loopback, &web_file)
        .with_context(|| format!("{} nicht schreibbar", web_file.display()))?;

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| anyhow!("{} ist nicht verfügbar: {e}", args.listen))?;

    sys::kill_children_on_exit();
    let hub = hub::Hub::new(cfg.clone(), args.dir.is_some());
    let sink: Sink = {
        let hub = hub.clone();
        Arc::new(move |ev| hub.on_event(ev))
    };
    let cmd = engine::start(engine::Start {
        sink,
        base: base.clone(),
        cfg: cfg.clone(),
        // Its own file: the desktop app may run on the same machine.
        journal: Some(base.join("queue-web.json")),
        listener: None,
        launch: Launch::default(),
        check_updates: false,
    });

    let shutdown = CancellationToken::new();
    let app = Arc::new(api::App {
        hub,
        cmd,
        auth: auth::Auth::new(password.as_ref().map(|(p, _)| p.clone()), args.secure_cookie),
        stored_dir: args.dir.is_some().then_some(stored_dir),
        save_config: true,
        shutdown: shutdown.clone(),
    });

    println!("{}", banner(&args.listen, password.as_ref(), &web_file, &cfg));

    let server = axum::serve(listener, api::router(app)).with_graceful_shutdown(shutdown.clone().cancelled_owned());
    let mut server = tokio::spawn(server.into_future());
    tokio::select! {
        r = &mut server => return r.context("Server abgestürzt")?.context("Server beendet"),
        _ = stop_signal() => {}
    }
    println!("Beende … Unfertige Downloads laufen beim nächsten Start weiter.");
    shutdown.cancel();
    // Running file downloads get a moment; the queue is safe in queue-web.json either way.
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    Ok(())
}

/// Ctrl+C everywhere, SIGTERM on Unix (`docker stop`, systemd).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn banner(listen: &SocketAddr, password: Option<&(String, auth::Origin)>, web_file: &std::path::Path, cfg: &Config) -> String {
    let mut out = String::from("\nomnidl Web-Interface läuft:\n");
    for url in net::urls(listen) {
        out.push_str(&format!("  → {url}\n"));
    }
    out.push('\n');
    match password {
        None => out.push_str(
            "Ohne Passwort – nur von diesem Rechner aus erreichbar.\n\
             Für das Heimnetz: --listen 0.0.0.0:8080 (erzeugt ein Passwort).\n",
        ),
        Some((_, auth::Origin::Given)) => out.push_str("Anmeldung mit dem Passwort aus --password bzw. OMNIDL_PASSWORD.\n"),
        Some((p, origin)) => {
            let how = if *origin == auth::Origin::Generated { "neu erzeugt, gespeichert in" } else { "gespeichert in" };
            out.push_str(&format!("Passwort: {p}\n  ({how} {})\n", web_file.display()));
        }
    }
    out.push_str(&format!("Downloads: {}\n", cfg.download_dir.display()));
    out.push_str("Beenden mit Strg+C.\n");
    out
}
