#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bridge;
mod config;
mod deps;
mod detect;
mod engine;
mod extension;
mod gallery;
mod job;
mod journal;
mod launch;
mod matcher;
mod peek;
mod schedule;
mod spotify;
mod sys;
mod tagger;
mod theme;
mod update;
mod util;
mod widgets;
mod ytdlp;

use bridge::Claim;

fn main() -> eframe::Result {
    let launch = launch::parse(std::env::args().skip(1));
    if launch.uninstall {
        config::remove_app_files(&config::base_dir());
        return Ok(());
    }

    // One omnidl at a time: a second start hands its links over and quits.
    let listener = match bridge::claim(&launch) {
        Claim::Owner(l) => Ok(l),
        Claim::Forwarded => return Ok(()),
        Claim::Unavailable(e) => Err(e),
    };

    sys::kill_children_on_exit();
    if let Ok(exe) = std::env::current_exe() {
        std::thread::spawn(move || {
            // Development builds leave the link handler to the installed app.
            if !cfg!(debug_assertions) {
                let _ = sys::register_link_handler(&exe);
            }
            update::cleanup_when_free(&exe);
        });
    }

    let cfg = config::Config::load();
    let base = config::base_dir();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("omnidl")
            .with_inner_size([860.0, 620.0])
            .with_min_inner_size([620.0, 420.0])
            .with_icon(icon()),
        ..Default::default()
    };

    eframe::run_native(
        "omnidl",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, cfg, base, listener, launch)))),
    )
}

/// Window icon, rendered by `packaging/make_icons.py` as raw RGBA.
fn icon() -> egui::IconData {
    const S: u32 = 128;
    egui::IconData { rgba: include_bytes!("../assets/icon-128.rgba").to_vec(), width: S, height: S }
}
