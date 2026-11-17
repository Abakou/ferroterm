//! Apparence de l'application : palette, polices, style egui et composants
//! dessinés à la main (onglets, lignes de liste, boutons).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::win_frame::CaptionButton;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Rect, Response,
    RichText, Sense, Stroke, TextStyle, Vec2,
};

// --- Palette ---------------------------------------------------------------
// Les couleurs sont lues dans la palette courante (`p()`), commutable à chaud.

#[derive(Clone, Copy)]
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color32,
    pub sidebar: Color32,
    pub title_bar: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub red: Color32,
    pub teal: Color32,
    pub terminal_bg: Color32,
    pub terminal_fg: Color32,
    /// Les 16 couleurs ANSI du terminal.
    pub ansi: [Color32; 16],
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const DARK: Palette = Palette {
    name: "Orage",
    dark: true,
    bg: rgb(0x0d1117),
    sidebar: rgb(0x12161e),
    title_bar: rgb(0x0f131a),
    surface: rgb(0x191f2a),
    surface_hover: rgb(0x222a38),
    border: rgb(0x2a3240),
    text: rgb(0xe6edf3),
    text_dim: rgb(0x8b94a3),
    accent: rgb(0x4c8df6),
    accent_hover: rgb(0x6aa1f8),
    accent_soft: rgb(0x1b2c4a),
    green: rgb(0x3fb950),
    yellow: rgb(0xd29922),
    red: rgb(0xf85149),
    teal: rgb(0x2fd3b5),
    terminal_bg: rgb(0x0d1117),
    terminal_fg: rgb(0xe6edf3),
    ansi: [
        rgb(0x484f58), rgb(0xff7b72), rgb(0x3fb950), rgb(0xd29922), rgb(0x58a6ff), rgb(0xbc8cff), rgb(0x39c5cf), rgb(0xb1bac4),
        rgb(0x6e7681), rgb(0xffa198), rgb(0x56d364), rgb(0xe3b341), rgb(0x79c0ff), rgb(0xd2a8ff), rgb(0x56d4dd), rgb(0xf0f6fc),
    ],
};

pub const LIGHT: Palette = Palette {
    name: "Clair",
    dark: false,
    bg: rgb(0xffffff),
    sidebar: rgb(0xf3f4f6),
    title_bar: rgb(0xe9ebef),
    surface: rgb(0xf0f1f4),
    surface_hover: rgb(0xe2e5ea),
    border: rgb(0xd0d4da),
    text: rgb(0x1f2328),
    text_dim: rgb(0x656d76),
    accent: rgb(0x0969da),
    accent_hover: rgb(0x0550ae),
    accent_soft: rgb(0xddeafc),
    green: rgb(0x1a7f37),
    yellow: rgb(0x9a6700),
    red: rgb(0xcf222e),
    teal: rgb(0x0d9488),
    terminal_bg: rgb(0xffffff),
    terminal_fg: rgb(0x1f2328),
    ansi: [
        rgb(0x24292f), rgb(0xcf222e), rgb(0x116329), rgb(0x4d2d00), rgb(0x0969da), rgb(0x8250df), rgb(0x1b7c83), rgb(0x6e7781),
        rgb(0x57606a), rgb(0xa40e26), rgb(0x1a7f37), rgb(0x633c01), rgb(0x218bff), rgb(0xa475f9), rgb(0x3192aa), rgb(0x8c959f),
    ],
};

pub const NORD: Palette = Palette {
    name: "Nord",
    dark: true,
    bg: rgb(0x2e3440),
    sidebar: rgb(0x272c36),
    title_bar: rgb(0x242933),
    surface: rgb(0x3b4252),
    surface_hover: rgb(0x434c5e),
    border: rgb(0x4c566a),
    text: rgb(0xeceff4),
    text_dim: rgb(0x9aa4b5),
    accent: rgb(0x88c0d0),
    accent_hover: rgb(0x8fbcbb),
    accent_soft: rgb(0x3b4a5a),
    green: rgb(0xa3be8c),
    yellow: rgb(0xebcb8b),
    red: rgb(0xbf616a),
    teal: rgb(0x8fbcbb),
    terminal_bg: rgb(0x2e3440),
    terminal_fg: rgb(0xd8dee9),
    ansi: [
        rgb(0x3b4252), rgb(0xbf616a), rgb(0xa3be8c), rgb(0xebcb8b), rgb(0x81a1c1), rgb(0xb48ead), rgb(0x88c0d0), rgb(0xe5e9f0),
        rgb(0x4c566a), rgb(0xbf616a), rgb(0xa3be8c), rgb(0xebcb8b), rgb(0x81a1c1), rgb(0xb48ead), rgb(0x8fbcbb), rgb(0xeceff4),
    ],
};

pub const DRACULA: Palette = Palette {
    name: "Dracula",
    dark: true,
    bg: rgb(0x282a36),
    sidebar: rgb(0x21222c),
    title_bar: rgb(0x1e1f29),
    surface: rgb(0x343746),
    surface_hover: rgb(0x44475a),
    border: rgb(0x44475a),
    text: rgb(0xf8f8f2),
    text_dim: rgb(0xa0a3b0),
    accent: rgb(0xbd93f9),
    accent_hover: rgb(0xcaa9fa),
    accent_soft: rgb(0x3d3f55),
    green: rgb(0x50fa7b),
    yellow: rgb(0xf1fa8c),
    red: rgb(0xff5555),
    teal: rgb(0x8be9fd),
    terminal_bg: rgb(0x282a36),
    terminal_fg: rgb(0xf8f8f2),
    ansi: [
        rgb(0x21222c), rgb(0xff5555), rgb(0x50fa7b), rgb(0xf1fa8c), rgb(0xbd93f9), rgb(0xff79c6), rgb(0x8be9fd), rgb(0xf8f8f2),
        rgb(0x6272a4), rgb(0xff6e6e), rgb(0x69ff94), rgb(0xffffa5), rgb(0xd6acff), rgb(0xff92df), rgb(0xa4ffff), rgb(0xffffff),
    ],
};

/// Couleurs du thème « Dark Modern » de VS Code.
pub const VSCODE: Palette = Palette {
    name: "Sombre",
    dark: true,
    bg: rgb(0x1f1f1f),
    sidebar: rgb(0x181818),
    title_bar: rgb(0x181818),
    surface: rgb(0x313131),
    surface_hover: rgb(0x2a2d2e),
    border: rgb(0x2b2b2b),
    text: rgb(0xcccccc),
    text_dim: rgb(0x9d9d9d),
    accent: rgb(0x0078d4),
    accent_hover: rgb(0x4daafc),
    accent_soft: rgb(0x04395e),
    green: rgb(0x89d185),
    yellow: rgb(0xcca700),
    red: rgb(0xf14c4c),
    teal: rgb(0x29b8db),
    terminal_bg: rgb(0x1f1f1f),
    terminal_fg: rgb(0xcccccc),
    ansi: [
        rgb(0x000000), rgb(0xcd3131), rgb(0x0dbc79), rgb(0xe5e510), rgb(0x2472c8), rgb(0xbc3fbc), rgb(0x11a8cd), rgb(0xe5e5e5),
        rgb(0x666666), rgb(0xf14c4c), rgb(0x23d18b), rgb(0xf5f543), rgb(0x3b8eea), rgb(0xd670d6), rgb(0x29b8db), rgb(0xe5e5e5),
    ],
};

pub const THEMES: [Palette; 5] = [VSCODE, DARK, LIGHT, NORD, DRACULA];

static CURRENT: std::sync::RwLock<Palette> = std::sync::RwLock::new(VSCODE);

// Les dimensions et les polices sont communes à tous les thèmes (mesures
// tirées du CSS de VS Code) : une palette ne change que les couleurs.
/// Rayon des boutons et champs.
const RADIUS: u8 = 4;
/// Rayon des lignes de liste et en-têtes.
const ROW_RADIUS: f32 = 4.0;

/// Palette courante (copie, bon marché).
pub fn p() -> Palette {
    *CURRENT.read().expect("palette")
}

pub fn current_name() -> &'static str {
    p().name
}

/// Change de thème par son nom et réapplique le style. Renvoie faux si inconnu.
pub fn set_theme(ctx: &egui::Context, name: &str) -> bool {
    let Some(palette) = THEMES.iter().find(|t| t.name.eq_ignore_ascii_case(name)) else {
        return false;
    };
    *CURRENT.write().expect("palette") = *palette;
    apply(ctx);
    true
}

/// Applique la palette courante au style egui.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(if p().dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(apply_style);
}


// --- Icônes ----------------------------------------------------------------
// Segoe Fluent Icons, la police d'icônes de Windows, quand elle est présente.
// Sinon (Linux, macOS), Codicons, la police d'icônes de VS Code, embarquée
// (licence CC BY 4.0, voir `assets/codicon-LICENSE.txt`).
static ICON_FONT: AtomicBool = AtomicBool::new(false);

const CODICONS: &[u8] = include_bytes!("../assets/codicon.ttf");

/// Icône : point de code Segoe Fluent, puis point de code Codicons.
pub struct Icon(&'static str, &'static str);

impl Icon {
    pub fn get(&self) -> &'static str {
        if ICON_FONT.load(Ordering::Relaxed) { self.0 } else { self.1 }
    }
}

pub const ICON_ADD: Icon = Icon("\u{e710}", "\u{ea60}");
pub const ICON_CLOSE: Icon = Icon("\u{e711}", "\u{ea76}");
pub const ICON_SEARCH: Icon = Icon("\u{e721}", "\u{ea6d}");
pub const ICON_FOLDER: Icon = Icon("\u{e8b7}", "\u{ea83}");
pub const ICON_NEW_FOLDER: Icon = Icon("\u{e8f4}", "\u{ea80}");
pub const ICON_FILE: Icon = Icon("\u{e7c3}", "\u{ea7b}");
pub const ICON_LINK: Icon = Icon("\u{e71b}", "\u{eb15}");
pub const ICON_TERMINAL: Icon = Icon("\u{e756}", "\u{ea85}");
pub const ICON_SERVER: Icon = Icon("\u{e7f4}", "\u{eb50}");
pub const ICON_DOWNLOAD: Icon = Icon("\u{e896}", "\u{eac2}");
pub const ICON_UPLOAD: Icon = Icon("\u{e898}", "\u{eac3}");
pub const ICON_REFRESH: Icon = Icon("\u{e72c}", "\u{eb37}");
pub const ICON_UP: Icon = Icon("\u{e74a}", "\u{eaa1}");
pub const ICON_DELETE: Icon = Icon("\u{e74d}", "\u{ea81}");
pub const ICON_EDIT: Icon = Icon("\u{e70f}", "\u{ea73}");
pub const ICON_COPY: Icon = Icon("\u{e8c8}", "\u{ebcc}");
pub const ICON_PLAY: Icon = Icon("\u{e768}", "\u{eb2c}");
pub const ICON_IMPORT: Icon = Icon("\u{e8b5}", "\u{ea6f}");
pub const ICON_MACRO: Icon = Icon("\u{e945}", "\u{ea86}");

/// Texte « icône + libellé ».
pub fn label(icon: &Icon, text: &str) -> String {
    format!("{}  {text}", icon.get())
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

// --- Installation ----------------------------------------------------------

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    apply(ctx);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"C:\Windows".into())
        .join("Fonts");
    let mut load = |name: &str, files: &[&str]| -> bool {
        for file in files {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let has_ui = load("ui", &["segoeui.ttf"]);
    let has_semibold = load("ui-semibold", &["seguisb.ttf", "segoeuib.ttf"]);
    let has_mono = load("mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let has_segoe_icons = load("icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    ICON_FONT.store(has_segoe_icons, Ordering::Relaxed);
    if !has_segoe_icons {
        fonts.font_data.insert("icons".to_string(), Arc::new(FontData::from_static(CODICONS)));
    }

    let mut proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // Juste après la police du texte : les icônes priment sur les emojis d'egui,
    // sans que la police d'icônes ne fournisse les caractères ordinaires.
    proportional.insert(1.min(proportional.len()), "icons".into());
    if has_ui {
        proportional.insert(0, "ui".into());
    }
    let mut bold = proportional.clone();
    if has_semibold {
        bold.insert(0, "ui-semibold".into());
    }
    let mut mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    if has_mono {
        mono.insert(0, "mono".into());
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, mono);
    fonts.families.insert(semibold(), bold);
    ctx.set_fonts(fonts);
}

fn apply_style(style: &mut egui::Style) {
    let pal = p();
    let body = 13.0;
    style.text_styles = [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(body, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(body, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(20.0, semibold())),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0);
    s.button_padding = Vec2::new(10.0, 4.0);
    s.interact_size.y = 26.0;
    s.window_margin = Margin::same(18);
    s.menu_margin = Margin::same(8);
    s.scroll.bar_width = 8.0;
    s.scroll.floating = true;

    let v = &mut style.visuals;
    v.override_text_color = Some(p().text);
    v.panel_fill = p().bg;
    v.window_fill = pal.bg;
    v.window_stroke = Stroke::new(1.0, p().border);
    v.window_corner_radius = CornerRadius::same(RADIUS + 4);
    v.menu_corner_radius = CornerRadius::same(RADIUS + 2);
    v.extreme_bg_color = pal.surface;
    v.faint_bg_color = p().surface;
    v.selection.bg_fill = p().accent_soft;
    v.selection.stroke = Stroke::new(1.0, p().accent);
    v.hyperlink_color = p().accent;

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p().surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p().border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p().text_dim);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = p().surface;
    w.inactive.weak_bg_fill = p().surface;
    w.inactive.bg_stroke = Stroke::new(1.0, p().border);
    w.inactive.fg_stroke = Stroke::new(1.0, p().text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = p().surface_hover;
    w.hovered.weak_bg_fill = p().surface_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x45, 0x58));
    w.hovered.fg_stroke = Stroke::new(1.0, p().text);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p().accent_soft;
    w.active.weak_bg_fill = p().accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, p().accent);
    w.active.fg_stroke = Stroke::new(1.0, p().text);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = p().surface_hover;
    w.open.weak_bg_fill = p().surface_hover;
    w.open.bg_stroke = Stroke::new(1.0, p().accent);
    w.open.corner_radius = radius;
}

// --- Composants ------------------------------------------------------------

/// Bouton d'action principale (fond accentué).
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> Response {
    let fill = p().accent;
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.hovered.weak_bg_fill = p().accent_hover;
        w.hovered.bg_stroke = Stroke::NONE;
        w.active.weak_bg_fill = p().accent_hover;
        ui.add(
            egui::Button::new(RichText::new(text).color(Color32::WHITE).family(semibold()))
                .fill(fill)
                .stroke(Stroke::NONE),
        )
    })
    .inner
}

/// Petit bouton carré ne contenant qu'une icône.
pub fn icon_button(ui: &mut egui::Ui, icon: &Icon, tooltip: &str) -> Response {
    ui.add(egui::Button::new(icon.get()).min_size(Vec2::splat(30.0)))
        .on_hover_text(tooltip)
}

/// En-tête de panneau : titre de section à gauche, petits boutons d'action
/// (ajoutés par `actions`, de droite à gauche) alignés à droite.
pub fn panel_header(ui: &mut egui::Ui, title: &str, actions: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(section(title));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            actions(ui);
        });
    });
}

/// Petit bouton d'en-tête : icône seule, sans fond hors survol.
pub fn header_button(ui: &mut egui::Ui, icon: &Icon, tooltip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(icon.get()).size(14.0).color(p().text_dim))
            .frame_when_inactive(false)
            .min_size(Vec2::splat(22.0)),
    )
    .on_hover_text(tooltip)
}

/// Bouton d'en-tête dont l'icône porte une pastille « + » en bas à droite
/// (même dessin que l'icône « nouveau dossier »).
pub fn header_button_badged(ui: &mut egui::Ui, icon: &Icon, tooltip: &str) -> Response {
    let response = header_button(ui, icon, tooltip);
    if ui.is_rect_visible(response.rect) {
        let painter = ui.painter();
        let center = response.rect.center() + Vec2::new(4.5, 4.0);
        let radius = 3.6;
        // Liseré de la couleur du fond : détache la pastille de l'icône.
        painter.circle_filled(center, radius + 1.2, p().sidebar);
        painter.circle_filled(center, radius, p().text_dim);
        let stroke = egui::Stroke::new(1.0, p().sidebar);
        let arm = 2.0;
        painter.line_segment([center - Vec2::new(arm, 0.0), center + Vec2::new(arm, 0.0)], stroke);
        painter.line_segment([center - Vec2::new(0.0, arm), center + Vec2::new(0.0, arm)], stroke);
    }
    response
}

/// Ligne de liste pleine largeur : icône, titre et sous-titre optionnel.
pub fn list_row(ui: &mut egui::Ui, selected: bool, icon: &Icon, title: &str, subtitle: Option<&str>) -> Response {
    list_row_badge(ui, selected, icon, title, None, subtitle)
}

/// Comme [`list_row`], avec une précision en gris après le titre (ex. profil actif).
pub fn list_row_badge(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &Icon,
    title: &str,
    badge: Option<&str>,
    subtitle: Option<&str>,
) -> Response {
    let height = if subtitle.is_some() { 38.0 } else { 24.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // Désactivée (ex. dossier en cours de chargement) : couleurs atténuées.
    let fade = if ui.is_enabled() { 1.0 } else { 0.4 };
    if selected {
        // Ligne active survolée : même teinte, en partie transparente.
        let alpha = if response.hovered() { 0.65 } else { 1.0 };
        painter.rect_filled(rect, ROW_RADIUS, p().accent_soft.gamma_multiply(alpha));
    } else if response.hovered() {
        painter.rect_filled(rect, ROW_RADIUS, p().surface_hover);
    }
    let icon_color = if selected { p().accent_hover } else { p().text_dim }.gamma_multiply(fade);
    painter.text(
        rect.left_center() + Vec2::new(18.0, 0.0),
        Align2::CENTER_CENTER,
        icon.get(),
        FontId::proportional(14.0),
        icon_color,
    );
    let x = rect.left() + 36.0;
    match subtitle {
        Some(subtitle) => {
            let title_rect = painter.text(
                egui::pos2(x, rect.top() + 4.0),
                Align2::LEFT_TOP,
                title,
                FontId::proportional(12.0),
                p().text.gamma_multiply(fade),
            );
            if let Some(badge) = badge {
                painter.text(
                    egui::pos2(title_rect.right() + 6.0, title_rect.bottom()),
                    Align2::LEFT_BOTTOM,
                    badge,
                    FontId::proportional(11.0),
                    p().text_dim.gamma_multiply(fade),
                );
            }
            painter.text(
                egui::pos2(x, rect.bottom() - 5.0),
                Align2::LEFT_BOTTOM,
                subtitle,
                FontId::proportional(10.5),
                p().text_dim.gamma_multiply(fade),
            );
        }
        None => {
            painter.text(
                egui::pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(12.0),
                p().text.gamma_multiply(fade),
            );
        }
    }
    response
}

pub struct TabResponse {
    pub clicked: bool,
    pub close: bool,
    /// Réponse de l'onglet lui-même (menu contextuel, etc.).
    pub response: Response,
}

/// Onglet de la barre d'onglets : pastille d'état, titre, croix de fermeture.
/// `active` : onglet actif de son groupe ; `focused` : son groupe a le focus.
/// `max_width` : largeur maximale ; en style VS Code la largeur minimale est de
/// 120 px et les onglets ne rétrécissent pas, la barre défile.
pub fn tab(
    ui: &mut egui::Ui,
    active: bool,
    focused: bool,
    dot: Color32,
    icon: Option<&Icon>,
    title: &str,
    max_width: f32,
) -> TabResponse {
    let pal = p();
    let font = FontId::new(13.0, FontFamily::Proportional);
    let text_color = if active { pal.text } else { pal.text_dim };
    let galley = ui.painter().layout_no_wrap(title.to_string(), font, text_color);
    // Sans icône (ni pastille), le titre commence au bord gauche de l'onglet.
    let text_left = if icon.is_some() { 34.0 } else { 12.0 };
    let natural = text_left + galley.size().x + 8.0 + 28.0;
    let width = natural.max(120.0).min(max_width.max(90.0));
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 35.0), Sense::click_and_drag());
    let close_rect = Rect::from_center_size(rect.right_center() - Vec2::new(14.0, 0.0), Vec2::splat(20.0));
    let close = ui.interact(close_rect, response.id.with("close"), Sense::click());

    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, 0.0, pal.bg);
        let line = if focused { pal.accent } else { pal.border };
        painter.rect_filled(Rect::from_min_size(rect.left_top(), Vec2::new(rect.width(), 1.0)), 0.0, line);
    } else if response.hovered() || close.hovered() {
        painter.rect_filled(rect, 0.0, pal.bg);
    }
    // Séparateur entre onglets jointifs.
    painter.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, pal.border));
    if let Some(icon) = icon {
        painter.text(
            rect.left_center() + Vec2::new(18.0, 0.0),
            Align2::CENTER_CENTER,
            icon.get(),
            FontId::proportional(14.0),
            text_color,
        );
        // Pastille d'état dans le coin de l'icône.
        painter.circle_filled(rect.left_center() + Vec2::new(25.0, 6.0), 4.0, if active { pal.bg } else { pal.sidebar });
        painter.circle_filled(rect.left_center() + Vec2::new(25.0, 6.0), 3.0, dot);
    }

    let text_clip = Rect::from_min_max(
        egui::pos2(rect.left() + text_left, rect.top()),
        egui::pos2(close_rect.left() - 4.0, rect.bottom()),
    );
    painter
        .with_clip_rect(text_clip)
        .galley(egui::pos2(text_clip.left(), rect.center().y - galley.size().y / 2.0), galley, text_color);

    if active || response.hovered() || close.hovered() {
        if close.hovered() {
            painter.rect_filled(close_rect, 4.0, pal.surface_hover);
        }
        painter.text(
            close_rect.center(),
            Align2::CENTER_CENTER,
            ICON_CLOSE.get(),
            FontId::proportional(13.0),
            if close.hovered() { pal.text } else { text_color },
        );
    }

    TabResponse {
        clicked: response.clicked(),
        close: close.clicked() || response.middle_clicked(),
        response,
    }
}

/// Vrai si une fenêtre modale a été affichée pendant l’image en cours.
static MODAL_SHOWN: AtomicBool = AtomicBool::new(false);

/// Fenêtre modale de l’application : cadre de menu, avec une marge intérieure confortable.
/// Le fond assombri d’egui couvrirait toute la surface, marge de l’ombre comprise :
/// il est rendu transparent (il bloque toujours les clics) et `FerroApp` dessine
/// le sien aux contours de la fenêtre (voir `take_modal_shown`).
pub fn modal(ctx: &egui::Context, id: egui::Id) -> egui::Modal {
    MODAL_SHOWN.store(true, Ordering::Relaxed);
    egui::Modal::new(id)
        .backdrop_color(Color32::TRANSPARENT)
        .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(22)))
}

/// Indique si une modale a été affichée depuis le dernier appel, et remet l’indicateur à zéro.
pub fn take_modal_shown() -> bool {
    MODAL_SHOWN.swap(false, Ordering::Relaxed)
}

/// Couleur du fond assombri derrière une fenêtre modale (celle d’egui par défaut).
pub const MODAL_BACKDROP: Color32 = Color32::from_black_alpha(100);

/// Titre de section en petites capitales grises.
pub fn section(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).size(11.0).color(p().text).family(semibold())
}

/// Pastille flottante (état de connexion, reconnexion), centrée en bas de
/// `area` et posée par-dessus son contenu, avec une ombre portée.
pub fn status_pill(ui: &egui::Ui, id: egui::Id, area: Rect, add: impl FnOnce(&mut egui::Ui)) {
    egui::Area::new(id)
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(area.center_bottom() - Vec2::new(0.0, 20.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(p().surface)
                .stroke(Stroke::new(1.0, p().border))
                .corner_radius(12)
                .shadow(egui::Shadow {
                    offset: [0, 6],
                    blur: 20,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                })
                .inner_margin(Margin { left: 14, right: 8, top: 7, bottom: 7 })
                .show(ui, |ui| {
                    // Un message long passe à la ligne plutôt que de déborder du terminal.
                    ui.set_max_width((area.width() - 60.0).clamp(120.0, 620.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        add(ui);
                    });
                });
        });
}

/// Petit point de couleur, aligné sur le texte d'une ligne.
pub fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 18.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

// --- Barre de titre intégrée -------------------------------------------------
// --- Zones en cartes arrondies, comme les panneaux de VS Code ---------------
/// Rayon des coins d'une zone (barre latérale, terminal, fichiers).
pub const CARD_RADIUS: u8 = 8;
/// Espace entre deux zones, et entre une zone et le bord de la fenêtre.
pub const CARD_GAP: i8 = 5;

/// Cadre d'une zone : fond, bordure fine et coins arrondis, posé sur le fond
/// du plan de travail (`title_bar`).
pub fn card(fill: Color32, inner: Margin, outer: Margin) -> egui::Frame {
    egui::Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, p().border))
        .corner_radius(CARD_RADIUS)
        .inner_margin(inner)
        .outer_margin(outer)
}

/// Rectangle extérieur (bordure comprise) de la carte dont `ui` est le contenu.
/// À appeler en tout début de contenu, avec la même marge intérieure que `card`.
pub fn card_rect(ui: &egui::Ui, inner: Margin) -> Rect {
    (ui.max_rect() + inner).expand(1.0)
}

/// Rétablit les coins arrondis d'une carte une fois son contenu peint : les
/// fonds rectangulaires du contenu (barre d'onglets, terminal…) débordent sur
/// les coins, on les recouvre avec le fond du plan de travail puis on retrace
/// la bordure.
pub fn card_corners(ui: &egui::Ui, rect: Rect) {
    let radius = CARD_RADIUS as f32;
    // Le coin d'un carré dépasse de r·(√2 − 1) ≈ 0,41·r de l'arc.
    let cover = (radius * 0.42).ceil();
    let painter = ui.ctx().layer_painter(ui.layer_id());
    let size = Vec2::splat(radius + cover);
    for corner in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()] {
        // Uniquement le quart de ce coin, sans sortir de la carte : rien ne
        // doit déborder sur les voisins (boutons de la barre de titre…).
        let clip = Rect::from_center_size(corner, size * 2.0).intersect(rect);
        let painter = painter.with_clip_rect(clip);
        painter.rect_stroke(rect, CARD_RADIUS, Stroke::new(cover, p().title_bar), egui::StrokeKind::Outside);
        painter.rect_stroke(rect, CARD_RADIUS, Stroke::new(1.0, p().border), egui::StrokeKind::Inside);
    }
}

/// Poignée de redimensionnement : trois points verticaux au milieu de
/// l'espace qui sépare la carte de sa voisine, comme dans VS Code.
/// `right` : l'espace est à droite de la carte (sinon à gauche).
///
/// Au survol de la zone de redimensionnement, une barre accentuée remplit cet
/// espace sur toute la hauteur : après `SASH_HOVER_DELAY`, ou tout de suite
/// dès qu'on clique.
///
/// `ui` est l'`Ui` parente dans laquelle le panneau `panel` vient d'être
/// affiché, `rect` le rectangle de sa carte (`card_rect`).
pub fn card_sash(ui: &egui::Ui, panel: &str, rect: Rect, right: bool) {
    let gap = CARD_GAP as f32;
    let x = if right { rect.right() + gap / 2.0 } else { rect.left() - gap / 2.0 };
    let painter = ui.ctx().layer_painter(ui.layer_id());

    // Identifiant de la poignée de redimensionnement du panneau egui.
    let id = ui.scope_id().with(panel).with("__resize");
    let response = ui.ctx().read_response(id);
    // Zone de saisie d'egui : centrée sur le bord extérieur du panneau.
    let edge = if right { rect.right() + gap } else { rect.left() - gap };
    let grab = ui.style().interaction.resize_grab_radius_side;
    let zone = Rect::from_x_y_ranges(edge - grab..=edge + grab, rect.y_range());
    let (pointer, down, origin) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down(), i.pointer.press_origin()));
    let pressed = response.as_ref().is_some_and(|r| r.dragged() || r.is_pointer_button_down_on())
        || (down && origin.is_some_and(|pos| zone.contains(pos)));
    let hovered = response.as_ref().is_some_and(|r| r.hovered())
        || (!down && pointer.is_some_and(|pos| zone.contains(pos)));
    let key = id.with("sash_hover");
    let now = ui.input(|i| i.time);
    let visible = if pressed {
        // Reste affichée après le clic tant que le pointeur ne quitte pas la zone.
        ui.data_mut(|d| d.insert_temp(key, now - SASH_HOVER_DELAY));
        true
    } else if hovered {
        let since = now - ui.data_mut(|d| *d.get_temp_mut_or(key, now));
        if since < SASH_HOVER_DELAY {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SASH_HOVER_DELAY - since));
        }
        since >= SASH_HOVER_DELAY
    } else {
        ui.data_mut(|d| d.remove::<f64>(key));
        false
    };

    if visible {
        let bar = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::new(4.0, rect.height()));
        painter.rect_filled(bar, 2.0, p().accent);
    } else {
        for offset in [-4.0, 0.0, 4.0] {
            painter.circle_filled(egui::pos2(x, rect.center().y + offset), 1.0, p().text_dim);
        }
    }
}

/// Délai de survol avant l'apparition de la barre de redimensionnement.
const SASH_HOVER_DELAY: f64 = 1.0;

/// Masque la ligne qu'egui trace lui-même au survol d'un bord de panneau
/// redimensionnable (remplacée par `card_sash`). Renvoie le style d'origine :
/// à rétablir dans le contenu du panneau, puis sur `ui` une fois le panneau affiché.
pub fn hide_panel_resize_line(ui: &mut egui::Ui) -> Arc<egui::Style> {
    let saved = ui.style().clone();
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.hovered.fg_stroke = Stroke::NONE;
    widgets.active.fg_stroke = Stroke::NONE;
    saved
}

pub const TITLE_BAR_HEIGHT: f32 = 36.0;
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

pub const ICON_WIN_MINIMIZE: Icon = Icon("\u{e921}", "\u{eaba}");
pub const ICON_WIN_MAXIMIZE: Icon = Icon("\u{e922}", "\u{eab9}");
pub const ICON_WIN_RESTORE: Icon = Icon("\u{e923}", "\u{eabb}");
pub const ICON_WIN_CLOSE: Icon = Icon("\u{e8bb}", "\u{eab8}");

/// Rayon des coins de la fenêtre quand l'application dessine son propre cadre
/// (hors Windows), comme VS Code sous Linux.
pub const WINDOW_RADIUS: u8 = 8;
/// Marge transparente autour de la fenêtre, où est peinte l'ombre portée.
pub const WINDOW_SHADOW: f32 = 14.0;

/// Ombre portée de la fenêtre, contenue dans la marge `WINDOW_SHADOW`.
pub fn window_shadow() -> egui::Shadow {
    egui::Shadow {
        offset: [0, 3],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(if p().dark { 110 } else { 60 }),
    }
}

/// Bouton de fenêtre (réduire, agrandir, fermer), au style des contrôles de
/// Windows 11 : 46 px de large, survol gris clair, croix rouge au survol.
/// L'icône vient de Segoe Fluent si la police est chargée, sinon elle est
/// dessinée en traits fins de 10 px, comme les codicons de VS Code.
/// L'état survolé/enfoncé vient du système (`win_frame`) ou d'egui en repli.
/// Hors Windows, les boutons prennent l'apparence de GNOME (voir `adwaita_button`).
pub fn window_button(ui: &mut egui::Ui, kind: CaptionButton, maximized: bool, hovered: bool, pressed: bool) -> Response {
    if !cfg!(windows) {
        return adwaita_button(ui, kind, maximized);
    }
    let is_close = kind == CaptionButton::Close;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(46.0, TITLE_BAR_HEIGHT), Sense::click());
    let hovered = hovered || response.hovered();
    let pressed = pressed || response.is_pointer_button_down_on();
    let background = match (is_close, hovered, pressed) {
        (true, _, true) => Some(CLOSE_HOVER.gamma_multiply(0.9)),
        (true, true, _) => Some(CLOSE_HOVER),
        (false, _, true) => Some(p().text.gamma_multiply(0.06)),
        (false, true, _) => Some(p().text.gamma_multiply(0.1)),
        _ => None,
    };
    if let Some(background) = background {
        ui.painter().rect_filled(rect, 0.0, background);
    }
    let color = match (is_close, hovered, pressed) {
        (true, _, true) => Color32::from_white_alpha(0xb0),
        (true, true, _) => Color32::WHITE,
        (_, _, true) => p().text_dim,
        _ => p().text,
    };

    if ICON_FONT.load(Ordering::Relaxed) {
        let icon = match kind {
            CaptionButton::Minimize => &ICON_WIN_MINIMIZE,
            CaptionButton::Maximize if maximized => &ICON_WIN_RESTORE,
            CaptionButton::Maximize => &ICON_WIN_MAXIMIZE,
            CaptionButton::Close => &ICON_WIN_CLOSE,
        };
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, icon.get(), FontId::proportional(10.0), color);
    } else {
        paint_caption_glyph(ui, rect.center(), 5.0, kind, maximized, color);
    }
    response
}

/// Bouton de fenêtre au style GNOME (Adwaita, Yaru d'Ubuntu) : pastille ronde
/// de 24 px affichée au survol, plus marquée au clic, icône
/// symbolique de 8 px. « Fermer » n'a pas de traitement particulier.
fn adwaita_button(ui: &mut egui::Ui, kind: CaptionButton, maximized: bool) -> Response {
    const DIAMETER: f32 = 24.0;
    const SPACING: f32 = 10.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(DIAMETER + SPACING, TITLE_BAR_HEIGHT), Sense::click());
    let center = rect.center();
    // Pas de fond au repos : la pastille n'apparaît qu'au survol.
    if response.is_pointer_button_down_on() {
        ui.painter().circle_filled(center, DIAMETER / 2.0, p().text.gamma_multiply(0.25));
    } else if response.hovered() {
        ui.painter().circle_filled(center, DIAMETER / 2.0, p().text.gamma_multiply(0.12));
    }
    paint_caption_glyph(ui, center, 4.0, kind, maximized, p().text);
    response
}

/// Glyphes des boutons de fenêtre en traits de 1 px, alignés sur la grille des
/// pixels physiques pour rester nets. `h` est le demi-côté du glyphe.
fn paint_caption_glyph(ui: &egui::Ui, center: egui::Pos2, h: f32, kind: CaptionButton, maximized: bool, color: Color32) {
    let painter = ui.painter();
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp + 0.5 / ppp;
    let stroke = Stroke::new(1.0, color);
    let c = egui::pos2(snap(center.x), snap(center.y));
    // Décalage du second cadre de l'icône « restaurer ».
    let o = (h * 0.4).round().max(2.0);
    match kind {
        CaptionButton::Minimize => {
            painter.hline((c.x - h)..=(c.x + h), c.y, stroke);
        }
        CaptionButton::Maximize if maximized => {
            // Deux fenêtres superposées : celle de devant, et le coin de celle de derrière.
            let front = Rect::from_min_max(egui::pos2(c.x - h, c.y - h + o), egui::pos2(c.x + h - o, c.y + h));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
            let back = [
                egui::pos2(c.x - h + o, c.y - h + o),
                egui::pos2(c.x - h + o, c.y - h),
                egui::pos2(c.x + h, c.y - h),
                egui::pos2(c.x + h, c.y + h - o),
                egui::pos2(c.x + h - o, c.y + h - o),
            ];
            painter.add(egui::Shape::line(back.to_vec(), stroke));
        }
        CaptionButton::Maximize => {
            let square = Rect::from_center_size(c, Vec2::splat(2.0 * h));
            painter.rect_stroke(square, 1.0, stroke, egui::StrokeKind::Middle);
        }
        CaptionButton::Close => {
            let d = h;
            painter.line_segment([egui::pos2(c.x - d, c.y - d), egui::pos2(c.x + d, c.y + d)], stroke);
            painter.line_segment([egui::pos2(c.x - d, c.y + d), egui::pos2(c.x + d, c.y - d)], stroke);
        }
    }
}

/// Bouton d'action compact de la barre de titre.
pub fn title_action(ui: &mut egui::Ui, icon: &Icon, tooltip: &str, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 26.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, p().surface_hover);
    }
    let color = if active {
        p().accent_hover
    } else if response.hovered() {
        p().text
    } else {
        p().text_dim
    };
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, icon.get(), FontId::proportional(14.0), color);
    response.on_hover_text(tooltip)
}

/// Glyphes des toggles de disposition, dessinés à la main pour avoir une
/// version contour (panneau masqué) et une version remplie (panneau visible),
/// comme les toggles de disposition de VS Code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToggleGlyph {
    /// Cadre avec volet gauche : panneau des sessions.
    PanelLeft,
    /// Cadre avec volet droit : explorateur de fichiers distants.
    PanelRight,
    /// Cadre avec volet bas : supervision distante.
    PanelBottom,
}

pub fn title_toggle(ui: &mut egui::Ui, glyph: ToggleGlyph, tooltip: &str, active: bool) -> Response {
    // Dimensions de VS Code : bouton de 22×22 (codicon de 16 px + 3 px de
    // marge), glyphe de 14×12 avec un volet de 5 px (4 px pour le bas).
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, p().surface_hover);
    }
    let color = if active || response.hovered() { p().text } else { p().text_dim };
    let stroke = Stroke::new(1.0, color);
    let painter = ui.painter();
    let frame = Rect::from_center_size(rect.center(), Vec2::new(14.0, 12.0));
    let pane_width = 5.0;
    let pane_height = 4.0;
    match glyph {
        ToggleGlyph::PanelLeft => {
            let split = frame.left() + pane_width;
            if active {
                let pane = Rect::from_min_max(frame.min, egui::pos2(split, frame.max.y));
                painter.rect_filled(pane, CornerRadius { nw: 2, sw: 2, ne: 0, se: 0 }, color);
            } else {
                painter.vline(split, frame.y_range(), stroke);
            }
        }
        ToggleGlyph::PanelRight => {
            let split = frame.right() - pane_width;
            if active {
                let pane = Rect::from_min_max(egui::pos2(split, frame.min.y), frame.max);
                painter.rect_filled(pane, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, color);
            } else {
                painter.vline(split, frame.y_range(), stroke);
            }
        }
        ToggleGlyph::PanelBottom => {
            let split = frame.bottom() - pane_height;
            if active {
                let pane = Rect::from_min_max(egui::pos2(frame.min.x, split), frame.max);
                painter.rect_filled(pane, CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 }, color);
            } else {
                painter.hline(frame.x_range(), split, stroke);
            }
        }
    }
    painter.rect_stroke(frame, 2.0, stroke, egui::StrokeKind::Inside);
    response.on_hover_text(tooltip)
}

pub const ICON_CLOCK: Icon = Icon("\u{e823}", "\u{ea82}");
pub const ICON_USER: Icon = Icon("\u{e77b}", "\u{eb99}");
pub const ICON_DISK: Icon = Icon("\u{eda2}", "\u{eace}");
pub const ICON_ARROW_UP: Icon = Icon("\u{e74a}", "\u{eaa1}");
pub const ICON_ARROW_DOWN: Icon = Icon("\u{e74b}", "\u{ea9a}");

pub const ICON_COLOR: Icon = Icon("\u{e790}", "\u{eb5c}");

pub const ICON_CHEVRON_DOWN: Icon = Icon("\u{e70d}", "\u{eab4}");
pub const ICON_CHEVRON_RIGHT: Icon = Icon("\u{e76c}", "\u{eab6}");

/// En-tête de groupe de sessions : chevron, dossier, nom et badge de compte.
/// Toute la ligne est cliquable ; `open` indique l'état déplié.
pub fn group_header(ui: &mut egui::Ui, name: &str, count: usize, open: bool, drop_target: bool) -> Response {
    let height = 24.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if drop_target && response.contains_pointer() {
        painter.rect_filled(rect, ROW_RADIUS, p().accent_soft);
        painter.rect_stroke(rect, ROW_RADIUS, Stroke::new(1.0, p().accent), egui::StrokeKind::Inside);
    } else if response.hovered() {
        painter.rect_filled(rect, ROW_RADIUS, p().surface_hover);
    }
    let chevron = if open { &ICON_CHEVRON_DOWN } else { &ICON_CHEVRON_RIGHT };
    painter.text(
        rect.left_center() + Vec2::new(12.0, 0.0),
        Align2::CENTER_CENTER,
        chevron.get(),
        FontId::proportional(9.0),
        p().text_dim,
    );
    painter.text(
        rect.left_center() + Vec2::new(32.0, 0.0),
        Align2::CENTER_CENTER,
        ICON_FOLDER.get(),
        FontId::proportional(14.0),
        if open { p().accent_hover } else { p().text_dim },
    );
    // Badge du nombre de sessions, aligné à droite.
    let badge_text = count.to_string();
    let badge_galley = painter.layout_no_wrap(badge_text, FontId::proportional(11.0), p().text_dim);
    let badge = Rect::from_center_size(
        rect.right_center() - Vec2::new(8.0 + badge_galley.size().x / 2.0 + 7.0, 0.0),
        Vec2::new(badge_galley.size().x + 14.0, 18.0),
    );
    painter.rect_filled(badge, 9.0, if response.hovered() { p().surface } else { p().surface_hover });
    painter.galley(badge.center() - badge_galley.size() / 2.0, badge_galley, p().text_dim);

    let name_clip = Rect::from_min_max(rect.left_top() + Vec2::new(48.0, 0.0), badge.left_bottom() - Vec2::new(8.0, 0.0));
    painter.with_clip_rect(name_clip).text(
        egui::pos2(name_clip.left(), rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::new(13.0, semibold()),
        p().text,
    );
    response
}

/// Élément de la barre d'activité (colonne d'icônes à gauche).
pub fn activity_item(ui: &mut egui::Ui, icon: &Icon, tooltip: &str, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(36.0, 40.0), Sense::click());
    let pal = p();
    if active {
        // Indicateur sur le bord gauche de la barre, comme dans VS Code.
        let bar = Rect::from_min_size(egui::pos2(ui.clip_rect().left(), rect.top()), Vec2::new(2.0, rect.height()));
        ui.painter().rect_filled(bar, 0.0, pal.accent);
    }
    let color = if active || response.hovered() { pal.text } else { pal.text_dim };
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, icon.get(), FontId::proportional(22.0), color);
    response.on_hover_text(tooltip)
}

/// Bouton « scinder » : deux volets côte à côte (ou empilés si `down`),
/// dessinés (pas de glyphe fiable dans la police d'icônes).
pub fn split_button(ui: &mut egui::Ui, tooltip: &str, down: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 26.0), Sense::click());
    let pal = p();
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, pal.surface_hover);
    }
    let color = if response.hovered() { pal.text } else { pal.text_dim };
    let stroke = Stroke::new(1.2, color);
    let frame = Rect::from_center_size(rect.center(), Vec2::new(16.0, 12.0));
    ui.painter().rect_stroke(frame, 2.0, stroke, egui::StrokeKind::Inside);
    if down {
        ui.painter().hline(frame.x_range(), frame.center().y, stroke);
    } else {
        ui.painter().vline(frame.center().x, frame.y_range(), stroke);
    }
    response.on_hover_text(tooltip)
}
