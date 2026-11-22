// Pas de console noire en version release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bundle;
mod edit;
mod ftp;
mod highlight;
mod local;
mod mobaxterm;
mod monitor;
mod session;
mod ssh;
mod terminal;
mod theme;
mod vault;
mod win_frame;

use eframe::egui;

/// Sans GPU utilisable (machine virtuelle sans accélération 3D : pas de nœud
/// de rendu `/dev/dri/renderD*`), Mesa essaie les pilotes matériels puis Zink
/// avant de se replier sur le rendu logiciel, en affichant au passage une
/// série d'avertissements EGL. On demande directement le rendu logiciel,
/// sauf si l'utilisateur a déjà fixé la variable lui-même.
#[cfg(target_os = "linux")]
fn prefer_software_gl_without_gpu() {
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some() {
        return;
    }
    let has_render_node = std::fs::read_dir("/dev/dri")
        .map(|dir| dir.flatten().any(|e| e.file_name().to_string_lossy().starts_with("renderD")))
        .unwrap_or(false);
    if !has_render_node {
        // SAFETY : appelé en tout début de `main`, avant la création de tout autre thread.
        unsafe { std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1") };
    }
}

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    prefer_software_gl_without_gpu();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Ferroterm")
            // Sous Linux, relie la fenêtre au lanceur `ferroterm.desktop` (app_id
            // Wayland, WM_CLASS X11) : c'est de lui que GNOME tire l'icône du dock.
            .with_app_id("ferroterm")
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../assets/ferroterm-64.rgba").to_vec(),
                width: 64,
                height: 64,
            }))
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0])
            // Sous Windows, le cadre système est conservé et la barre de titre y est
            // intégrée (voir `win_frame`). Ailleurs, on le retire : la barre de titre
            // d'egui fournit déplacement et boutons, et `FerroApp` gère les bords.
            .with_decorations(cfg!(windows))
            // Fond transparent hors Windows : coins arrondis et ombre portée.
            .with_transparent(!cfg!(windows)),
        ..Default::default()
    };
    eframe::run_native(
        "Ferroterm",
        options,
        Box::new(|cc| Ok(Box::new(app::FerroApp::new(cc)))),
    )
}
