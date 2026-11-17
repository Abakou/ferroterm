//! Barre de titre intégrée avec les contrôles natifs de Windows 11.
//!
//! Même technique que Windows Terminal ou l'Explorateur de fichiers : la fenêtre
//! garde son cadre système (ombre, coins arrondis, bordures de redimensionnement,
//! disposition Snap), mais la zone client est étendue jusqu'en haut
//! (`WM_NCCALCSIZE`) et l'application dit à Windows ce que représente chaque
//! zone de sa barre de titre (`WM_NCHITTEST`) :
//! - `HTCAPTION` : déplacement, double-clic pour agrandir, menu système au clic droit ;
//! - `HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE` : boutons de fenêtre, avec le menu
//!   de disposition Snap de Windows 11 au survol du bouton « agrandir » ;
//! - `HTCLIENT` : les menus et actions, qui restent gérés par egui.
//!
//! Hors Windows, `install` renvoie `false` et la barre se replie sur les
//! commandes de fenêtre d'egui.

use std::sync::Mutex;

use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

/// Zones de la barre de titre, en points logiques egui, mises à jour à chaque image.
#[derive(Default, Clone)]
pub struct Regions {
    /// Lu uniquement par le code natif Windows (`WM_NCHITTEST`).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub height: f32,
    /// Zones qui restent interactives pour egui (menus, actions).
    pub client: Vec<egui::Rect>,
    pub buttons: Vec<(CaptionButton, egui::Rect)>,
}

struct State {
    installed: bool,
    // Lus uniquement par le code natif Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
    #[cfg_attr(not(windows), allow(dead_code))]
    ctx: Option<egui::Context>,
    scale: f32,
    regions: Regions,
    hovered: Option<CaptionButton>,
    pressed: Option<CaptionButton>,
}

static STATE: Mutex<State> = Mutex::new(State {
    installed: false,
    hwnd: 0,
    ctx: None,
    scale: 1.0,
    regions: Regions { height: 0.0, client: Vec::new(), buttons: Vec::new() },
    hovered: None,
    pressed: None,
});

/// Trace de diagnostic : `FERROTERM_TRACE=<fichier>` journalise les messages
/// souris de la barre de titre et l'état lu à chaque dessin.
pub fn trace(message: impl FnOnce() -> String) {
    use std::io::Write;
    let Some(path) = std::env::var_os("FERROTERM_TRACE") else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() % 1_000_000)
            .unwrap_or(0);
        let _ = writeln!(file, "{now:6} {}", message());
    }
}

pub fn is_active() -> bool {
    STATE.lock().expect("win_frame").installed
}

/// À appeler à chaque image, après le dessin de la barre de titre.
pub fn update(regions: Regions, scale: f32) {
    let mut state = STATE.lock().expect("win_frame");
    state.regions = regions;
    state.scale = scale;
}

/// Bouton survolé et bouton enfoncé, pour le dessin.
pub fn caption_state() -> (Option<CaptionButton>, Option<CaptionButton>) {
    let state = STATE.lock().expect("win_frame");
    trace(|| format!("dessin : survolé={:?} enfoncé={:?}", state.hovered, state.pressed));
    (state.hovered, state.pressed)
}

#[cfg(windows)]
pub use native::{install, set_frame_theme};

#[cfg(not(windows))]
pub fn install(_cc: &eframe::CreationContext<'_>) -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_frame_theme(_dark: bool, _border: egui::Color32) {}

#[cfg(windows)]
mod native {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, HTCAPTION, HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        IsZoomed, NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SM_CXFRAME,
        SM_CXPADDEDBORDER, SM_CYFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetWindowPos, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
        WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND,
    };

    /// Active la barre de titre intégrée sur la fenêtre principale.
    pub fn install(cc: &eframe::CreationContext<'_>) -> bool {
        let Ok(handle) = cc.window_handle() else { return false };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else { return false };
        let hwnd: HWND = win32.hwnd.get();
        {
            let mut state = STATE.lock().expect("win_frame");
            state.ctx = Some(cc.egui_ctx.clone());
            state.hwnd = hwnd;
            state.scale = cc.egui_ctx.pixels_per_point();
        }
        // SAFETY : appels Win32 sur la fenêtre que winit vient de créer, depuis son thread.
        unsafe {
            if SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0) == 0 {
                return false;
            }
            let dark: i32 = 1;
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as _, 4);
            // Bordure du cadre assortie au thème (COLORREF = 0x00BBGGRR).
            let border = crate::theme::p().border;
            let color: u32 = (border.b() as u32) << 16 | (border.g() as u32) << 8 | border.r() as u32;
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &color as *const u32 as _, 4);
            // Force un recalcul de la zone client avec notre `WM_NCCALCSIZE`.
            SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        STATE.lock().expect("win_frame").installed = true;
        true
    }

    fn button_for_hit(hit: WPARAM) -> Option<CaptionButton> {
        match hit as u32 {
            HTMINBUTTON => Some(CaptionButton::Minimize),
            HTMAXBUTTON => Some(CaptionButton::Maximize),
            HTCLOSE => Some(CaptionButton::Close),
            _ => None,
        }
    }

    fn hit_for_button(button: CaptionButton) -> u32 {
        match button {
            CaptionButton::Minimize => HTMINBUTTON,
            CaptionButton::Maximize => HTMAXBUTTON,
            CaptionButton::Close => HTCLOSE,
        }
    }

    fn repaint(state: &State) {
        if let Some(ctx) = &state.ctx {
            ctx.request_repaint();
        }
    }

    /// Couleur de bordure et mode sombre/clair du cadre système (changement de thème).
    pub fn set_frame_theme(dark: bool, border: egui::Color32) {
        let hwnd = STATE.lock().expect("win_frame").hwnd;
        if hwnd == 0 {
            return;
        }
        let dark: i32 = dark as i32;
        let color: u32 = (border.b() as u32) << 16 | (border.g() as u32) << 8 | border.r() as u32;
        // SAFETY : attributs DWM sur notre propre fenêtre.
        unsafe {
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as _, 4);
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &color as *const u32 as _, 4);
        }
    }

    /// Épaisseur des bordures de redimensionnement (x, y), en pixels.
    unsafe fn frame_thickness(hwnd: HWND) -> (i32, i32) {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let padded = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
        (
            unsafe { GetSystemMetricsForDpi(SM_CXFRAME, dpi) } + padded,
            unsafe { GetSystemMetricsForDpi(SM_CYFRAME, dpi) } + padded,
        )
    }

    fn contains(rect: &egui::Rect, scale: f32, pt: POINT) -> bool {
        rect.contains(egui::pos2(pt.x as f32 / scale, pt.y as f32 / scale))
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // SAFETY : messages envoyés par Windows à notre fenêtre ; les pointeurs
        // reçus sont valides pendant le traitement du message.
        unsafe {
            match msg {
                WM_NCCALCSIZE if wparam != 0 => {
                    let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                    let top = params.rgrc[0].top;
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    // On garde les bordures gauche/droite/bas calculées par Windows
                    // et on supprime la barre de titre système. Une fenêtre agrandie
                    // déborde de l'écran de l'épaisseur du cadre : on la compense.
                    params.rgrc[0].top = if IsZoomed(hwnd) != 0 { top + frame_thickness(hwnd).1 } else { top };
                    result
                }
                WM_NCHITTEST => {
                    let result = DefSubclassProc(hwnd, msg, wparam, lparam);
                    if result != HTCLIENT as LRESULT {
                        return result;
                    }
                    let mut pt = POINT {
                        x: (lparam & 0xffff) as i16 as i32,
                        y: ((lparam >> 16) & 0xffff) as i16 as i32,
                    };
                    ScreenToClient(hwnd, &mut pt);
                    let state = STATE.lock().expect("win_frame");
                    if IsZoomed(hwnd) == 0 {
                        let (fx, fy) = frame_thickness(hwnd);
                        if pt.y < fy {
                            let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                            GetClientRect(hwnd, &mut client);
                            return if pt.x < fx {
                                HTTOPLEFT
                            } else if pt.x >= client.right - fx {
                                HTTOPRIGHT
                            } else {
                                HTTOP
                            } as LRESULT;
                        }
                    }
                    if (pt.y as f32) < state.regions.height * state.scale {
                        for (button, rect) in &state.regions.buttons {
                            if contains(rect, state.scale, pt) {
                                return hit_for_button(*button) as LRESULT;
                            }
                        }
                        if state.regions.client.iter().any(|r| contains(r, state.scale, pt)) {
                            return HTCLIENT as LRESULT;
                        }
                        return HTCAPTION as LRESULT;
                    }
                    HTCLIENT as LRESULT
                }
                WM_NCMOUSEMOVE => {
                    let hovered = button_for_hit(wparam);
                    trace(|| format!("WM_NCMOUSEMOVE hit={wparam} -> {hovered:?}"));
                    let mut state = STATE.lock().expect("win_frame");
                    if state.hovered != hovered {
                        state.hovered = hovered;
                        repaint(&state);
                    }
                    if hovered.is_some() {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE | TME_NONCLIENT,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        TrackMouseEvent(&mut track);
                    }
                    drop(state);
                    // Windows doit voir ce message : c'est lui qui affiche le menu
                    // de disposition Snap au survol du bouton « agrandir ».
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                WM_NCMOUSELEAVE | WM_MOUSEMOVE => {
                    let mut state = STATE.lock().expect("win_frame");
                    if state.hovered.is_some() || state.pressed.is_some() {
                        trace(|| format!("{} : fin du survol", if msg == WM_MOUSEMOVE { "WM_MOUSEMOVE" } else { "WM_NCMOUSELEAVE" }));
                        state.hovered = None;
                        state.pressed = None;
                        repaint(&state);
                    }
                    drop(state);
                    DefSubclassProc(hwnd, msg, wparam, lparam)
                }
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => match button_for_hit(wparam) {
                    Some(button) => {
                        let mut state = STATE.lock().expect("win_frame");
                        state.pressed = Some(button);
                        repaint(&state);
                        0
                    }
                    None => DefSubclassProc(hwnd, msg, wparam, lparam),
                },
                WM_NCLBUTTONUP => {
                    let pressed = {
                        let mut state = STATE.lock().expect("win_frame");
                        let pressed = state.pressed.take();
                        if pressed.is_some() {
                            repaint(&state);
                        }
                        pressed
                    };
                    match pressed {
                        Some(button) if button_for_hit(wparam) == Some(button) => {
                            let command = match button {
                                CaptionButton::Minimize => SC_MINIMIZE,
                                CaptionButton::Maximize if IsZoomed(hwnd) != 0 => SC_RESTORE,
                                CaptionButton::Maximize => SC_MAXIMIZE,
                                CaptionButton::Close => SC_CLOSE,
                            };
                            PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
                            0
                        }
                        Some(_) => 0,
                        None => DefSubclassProc(hwnd, msg, wparam, lparam),
                    }
                }
                _ => DefSubclassProc(hwnd, msg, wparam, lparam),
            }
        }
    }
}
