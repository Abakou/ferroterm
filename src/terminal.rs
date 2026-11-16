//! Affichage d'un terminal `vt100` dans egui, et traduction du clavier en
//! séquences envoyées au serveur.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, EventFilter, FontId, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextFormat, Vec2, text::LayoutJob,
};

const FONT_SIZE: f32 = 14.0;
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 40.0;
const PADDING: f32 = 10.0;

fn default_fg() -> Color32 {
    crate::theme::p().terminal_fg
}
fn default_bg() -> Color32 {
    crate::theme::p().terminal_bg
}
fn cursor_color() -> Color32 {
    crate::theme::p().accent
}
const SELECTION: Color32 = Color32::from_rgba_premultiplied(0x28, 0x3e, 0x5c, 0x70);

/// Surlignage des résultats de recherche ; le résultat courant est plus marqué.
fn match_color(current: bool) -> Color32 {
    if current {
        Color32::from_rgba_unmultiplied(0xff, 0x95, 0x00, 0xa0)
    } else {
        Color32::from_rgba_unmultiplied(0xfc, 0xe9, 0x4f, 0x48)
    }
}

/// Délai entre deux relectures de l'historique quand la recherche est ouverte
/// (la sortie du programme peut ajouter des résultats).
const SEARCH_REFRESH: Duration = Duration::from_millis(300);
const SEARCH_BAR_SIZE: Vec2 = Vec2::new(440.0, 34.0);
/// Champ de saisie, options comprises (elles sont dessinées dans son cadre).
const SEARCH_FIELD_WIDTH: f32 = 200.0;
/// Largeur minimale du champ quand la place manque.
const SEARCH_FIELD_MIN_WIDTH: f32 = 120.0;
const SEARCH_OPTION_SIZE: Vec2 = Vec2::new(22.0, 20.0);
/// Place réservée au compteur (« 42 sur 622 », « Aucun résultat »).
const SEARCH_COUNT_WIDTH: f32 = 76.0;


type CellPos = (u16, u16); // (ligne, colonne)

#[derive(Default)]
pub struct TerminalView {
    size: (u16, u16), // (lignes, colonnes)
    selection: Option<(CellPos, CellPos)>,
    scroll_accum: f32,
    /// Taille de police de ce terminal (zoom) ; `None` = taille normale.
    pub font_size: Option<f32>,
    /// Demande le focus clavier à la prochaine image (ex. après connexion).
    pub wants_focus: bool,
    /// Recherche dans le terminal (Ctrl+F) ; `None` = barre fermée.
    search: Option<Search>,
}

/// Résultat de recherche : ligne comptée depuis le haut de l'historique,
/// première et dernière colonnes (incluses).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Match {
    line: usize,
    start: u16,
    end: u16,
}

/// Critères d'une recherche : texte, casse, mot entier, expression régulière.
type SearchKey = (String, bool, bool, bool);

#[derive(Default)]
struct Search {
    query: String,
    matches: Vec<Match>,
    /// Indice du résultat courant dans `matches`.
    current: usize,
    /// Respecter la casse (majuscules / minuscules).
    case_sensitive: bool,
    /// Ne trouver que des mots entiers.
    whole_word: bool,
    /// Le texte saisi est une expression régulière.
    regex: bool,
    /// L'expression régulière saisie est invalide.
    invalid: bool,
    /// Critères de la dernière lecture de l'historique, et son instant.
    scanned: Option<(SearchKey, Instant)>,
    /// Donne le focus au champ de saisie à la prochaine image.
    focus: bool,
    /// Largeur de la barre, mesurée sur son contenu (0 = pas encore mesurée).
    bar_width: f32,
    /// Largeur du champ de saisie à l'image précédente.
    field_width: f32,
}

#[derive(Default)]
pub struct TerminalOutput {
    /// Octets à envoyer au serveur.
    pub input: Vec<u8>,
    /// Nouvelle taille (colonnes, lignes) à communiquer au serveur.
    pub resize: Option<(u16, u16)>,
    /// Le terminal a le focus clavier.
    pub focused: bool,
}

impl TerminalView {
    /// Ouvre la barre de recherche (ou lui redonne le focus si elle est ouverte).
    pub fn open_search(&mut self) {
        self.search.get_or_insert_with(Search::default).focus = true;
    }

    /// `highlighter` : coloration des mots-clés (`None` = désactivée).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        parser: &Mutex<vt100::Parser>,
        highlighter: Option<&crate::highlight::Highlighter>,
    ) -> TerminalOutput {
        let mut out = TerminalOutput::default();
        // Taille de police propre à ce terminal, réglée par Ctrl+molette.
        let font_size = self.font_size.unwrap_or(FONT_SIZE);
        let font_id = FontId::monospace(font_size);
        let (char_w, row_h) = ui
            .ctx()
            .fonts_mut(|f| (f.glyph_width(&font_id, 'M'), f.row_height(&font_id)));

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let inner = rect.shrink(PADDING);
        let cols = ((inner.width() / char_w).floor() as u16).max(10);
        let rows = ((inner.height() / row_h).floor() as u16).max(3);

        let mut parser = parser.lock().expect("parser");
        if self.size != (rows, cols) {
            parser.set_size(nonzero(rows), nonzero(cols));
            self.size = (rows, cols);
            self.selection = None;
            out.resize = Some((cols, rows));
        }

        // --- Focus ---
        if response.clicked() || response.drag_started() || response.secondary_clicked() || self.wants_focus {
            response.request_focus();
            self.wants_focus = false;
        }
        let has_focus = response.has_focus();
        out.focused = has_focus;
        if has_focus {
            // Garder Tab, Échap et les flèches pour le terminal.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                        ..Default::default()
                    },
                )
            });
        }

        let (app_cursor, bracketed, alternate) = {
            let s = parser.screen();
            (s.application_cursor(), s.bracketed_paste(), s.alternate_screen())
        };

        // --- Clavier ---
        if has_focus {
            let events = ui.input(|i| i.events.clone());
            let mut open_search = false;
            for event in events {
                match event {
                    egui::Event::Text(text) => out.input.extend_from_slice(text.as_bytes()),
                    egui::Event::Paste(text) => paste(&mut out.input, &text, bracketed),
                    // Comme dans MobaXterm : la sélection est copiée automatiquement,
                    // Ctrl+C reste l'interruption du programme distant.
                    egui::Event::Copy => out.input.push(0x03),
                    egui::Event::Cut => out.input.push(0x18),
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        if key == Key::F && modifiers.command && !modifiers.alt && (modifiers.shift || !alternate) {
                            // Ctrl+F ou Ctrl+Maj+F : recherche. En plein écran (vim, less…),
                            // Ctrl+F reste pour le programme, qui s'en sert pour avancer d'une page.
                            open_search = true;
                        // Maj+Page préc./suiv. : défilement local de l'historique.
                        } else if modifiers.shift && matches!(key, Key::PageUp | Key::PageDown) && !alternate {
                            let page = rows as i64 - 1;
                            let delta = if key == Key::PageUp { page } else { -page };
                            scroll_history(&mut parser, delta);
                        } else if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                            out.input.extend_from_slice(&bytes);
                        }
                    }
                    _ => {}
                }
            }
            if open_search {
                self.open_search();
            }
            if !out.input.is_empty() {
                parser.screen_mut().set_scrollback(0);
                self.selection = None;
            }
        }

        // --- Zoom : Ctrl+molette sur le terminal, Ctrl+0 pour revenir à la taille normale ---
        let mut new_size = font_size;
        if response.hovered() {
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 {
                new_size = (font_size * zoom).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
            }
        }
        if has_focus && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Num0)) {
            new_size = FONT_SIZE;
        }
        if new_size != font_size {
            self.font_size = Some(new_size);
            ui.ctx().request_repaint();
        }

        // --- Molette ---
        if response.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                self.scroll_accum += dy / row_h;
                let lines = self.scroll_accum.trunc() as i64;
                self.scroll_accum -= lines as f32;
                if lines != 0 {
                    if alternate {
                        // vim, less… : la molette envoie des flèches.
                        let seq: &[u8] = match (lines > 0, app_cursor) {
                            (true, true) => b"\x1bOA",
                            (true, false) => b"\x1b[A",
                            (false, true) => b"\x1bOB",
                            (false, false) => b"\x1b[B",
                        };
                        for _ in 0..lines.abs() {
                            out.input.extend_from_slice(seq);
                        }
                    } else {
                        scroll_history(&mut parser, lines);
                    }
                }
            }
        }

        // --- Sélection à la souris ---
        let to_cell = |pos: Pos2| -> CellPos {
            let col = ((pos.x - inner.left()) / char_w).floor().clamp(0.0, (cols - 1) as f32);
            let row = ((pos.y - inner.top()) / row_h).floor().clamp(0.0, (rows - 1) as f32);
            (row as u16, col as u16)
        };
        if response.drag_started_by(PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = to_cell(pos);
                self.selection = Some((cell, cell));
            }
        } else if response.dragged_by(PointerButton::Primary) {
            if let (Some(pos), Some(sel)) = (response.interact_pointer_pos(), self.selection.as_mut()) {
                sel.1 = to_cell(pos);
            }
        }
        if response.drag_stopped_by(PointerButton::Primary) {
            if let Some(text) = self.selected_text(parser.screen()) {
                if !text.trim().is_empty() {
                    ui.ctx().copy_text(text);
                }
            }
        }
        if response.clicked() {
            self.selection = None;
        }
        // Clic droit : coller (comportement MobaXterm).
        if response.secondary_clicked() {
            if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                paste(&mut out.input, &text, bracketed);
            }
        }

        // --- Mini barre de défilement de l'historique ---
        let offset = parser.screen().scrollback();
        let history = history_len(&mut parser);
        let mut scrollbar: Option<(Rect, bool)> = None;
        if history > 0 && !alternate {
            let track = Rect::from_min_max(
                egui::pos2(rect.right() - 8.0, rect.top() + 2.0),
                egui::pos2(rect.right() - 2.0, rect.bottom() - 2.0),
            );
            let thumb_h = (track.height() * rows as f32 / (rows as f32 + history as f32)).max(24.0).min(track.height());
            let travel = (track.height() - thumb_h).max(1.0);
            // Zone de saisie un peu plus large que la barre, sur toute la hauteur.
            let grab = ui.interact(track.expand2(Vec2::new(4.0, 0.0)), response.id.with("scrollbar"), Sense::click_and_drag());
            let mut offset = offset;
            if grab.dragged() || grab.is_pointer_button_down_on() {
                if let Some(pos) = grab.interact_pointer_pos() {
                    // Haut de la piste = lignes les plus anciennes.
                    let fraction = ((pos.y - track.top() - thumb_h / 2.0) / travel).clamp(0.0, 1.0);
                    offset = ((1.0 - fraction) * history as f32).round() as usize;
                    parser.screen_mut().set_scrollback(offset);
                    self.selection = None;
                }
            }
            let top = track.top() + travel * (1.0 - offset as f32 / history as f32);
            let thumb = Rect::from_min_size(egui::pos2(track.left(), top), Vec2::new(track.width(), thumb_h));
            scrollbar = Some((thumb, grab.hovered() || grab.dragged()));
        }

        // --- Dessin ---
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, default_bg());
        let screen = parser.screen();
        let cell_rect = |row: u16, col: u16, width: u16| {
            Rect::from_min_size(
                Pos2::new(inner.left() + col as f32 * char_w, inner.top() + row as f32 * row_h),
                Vec2::new(width as f32 * char_w, row_h),
            )
        };

        for row in 0..rows {
            // Mots-clés : couleur imposée aux cellules sans couleur propre.
            let keywords = highlighter.map(|h| row_keywords(screen, row, cols, h)).unwrap_or_default();
            let style_at = |cell: &vt100::Cell, col: u16| {
                let mut style = CellStyle::of(cell);
                if let Some(Some(color)) = keywords.get(col as usize) {
                    if cell.fgcolor() == vt100::Color::Default && !cell.inverse() {
                        style.fg = *color;
                    }
                }
                style
            };
            let mut col = 0;
            while col < cols {
                let Some(first) = screen.cell(row, col) else {
                    col += 1;
                    continue;
                };
                let style = style_at(first, col);
                let start = col;
                let mut text = String::new();
                // Regrouper les cellules de même style ; un caractère large
                // (2 colonnes) est dessiné seul pour garder l'alignement.
                while col < cols {
                    let Some(cell) = screen.cell(row, col) else { break };
                    if style_at(cell, col) != style {
                        break;
                    }
                    if cell.is_wide_continuation() {
                        col += 1;
                        continue;
                    }
                    if cell.is_wide() {
                        if col == start {
                            text.push_str(cell.contents());
                            col += 2;
                        }
                        break;
                    }
                    if cell.has_contents() {
                        text.push_str(cell.contents());
                    } else {
                        text.push(' ');
                    }
                    col += 1;
                }
                if col == start {
                    col += 1; // sécurité : toujours avancer
                    continue;
                }
                let run = cell_rect(row, start, col - start);
                if style.bg != default_bg() {
                    painter.rect_filled(run, 0.0, style.bg);
                }
                if !text.trim().is_empty() {
                    let format = TextFormat {
                        font_id: font_id.clone(),
                        color: style.fg,
                        italics: style.italic,
                        underline: if style.underline {
                            Stroke::new(1.0, style.fg)
                        } else {
                            Stroke::NONE
                        },
                        ..Default::default()
                    };
                    let galley = painter.layout_job(LayoutJob::single_section(text, format));
                    painter.galley(run.min, galley, style.fg);
                }
            }
        }

        // Sélection
        if let Some((start, end)) = self.ordered_selection() {
            for row in start.0..=end.0 {
                let from = if row == start.0 { start.1 } else { 0 };
                let to = if row == end.0 { end.1 } else { cols - 1 };
                painter.rect_filled(cell_rect(row, from, to - from + 1), 0.0, SELECTION);
            }
        }

        // Résultats de recherche visibles
        if let Some(search) = &self.search {
            let first = history - screen.scrollback().min(history);
            for (index, found) in search.matches.iter().enumerate() {
                if found.line < first || found.line >= first + rows as usize || found.start >= cols {
                    continue;
                }
                let row = (found.line - first) as u16;
                let end = found.end.min(cols - 1);
                painter.rect_filled(
                    cell_rect(row, found.start, end - found.start + 1),
                    0.0,
                    match_color(index == search.current),
                );
            }
        }

        // Curseur
        let scrolled = screen.scrollback();
        if !screen.hide_cursor() && scrolled == 0 {
            let (row, col) = screen.cursor_position();
            if row < rows && col < cols {
                let r = cell_rect(row, col, 1);
                if has_focus {
                    painter.rect_filled(r, 0.0, cursor_color().gamma_multiply(0.6));
                } else {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, cursor_color()), StrokeKind::Inside);
                }
            }
        }

        if let Some((thumb, active)) = scrollbar {
            let alpha = if active { 0.7 } else { 0.35 };
            painter.rect_filled(thumb, 3.0, crate::theme::p().text_dim.gamma_multiply(alpha));
        }

        // Indicateur d'historique
        if scrolled > 0 {
            painter.text(
                rect.right_top()
                    + Vec2::new(-16.0, if self.search.is_some() { SEARCH_BAR_SIZE.y + 14.0 } else { 6.0 }),
                egui::Align2::RIGHT_TOP,
                format!("↑ historique : {scrolled} lignes"),
                FontId::proportional(12.0),
                Color32::from_rgb(0xfc, 0xe9, 0x4f),
            );
        }

        // --- Barre de recherche, par-dessus le terminal, en haut à droite ---
        let mut close_search = false;
        if let Some(search) = self.search.as_mut() {
            // La barre épouse son contenu, mesuré à l'image précédente.
            // Si la place manque, le champ de saisie rétrécit jusqu'à sa largeur minimale.
            let rest = if search.bar_width > 0.0 {
                search.bar_width - search.field_width
            } else {
                SEARCH_BAR_SIZE.x - SEARCH_FIELD_WIDTH
            };
            let field_width =
                (rect.width() - 24.0 - rest).clamp(SEARCH_FIELD_MIN_WIDTH, SEARCH_FIELD_WIDTH).floor();
            let size = Vec2::new(rest + field_width, SEARCH_BAR_SIZE.y);
            let bar = Rect::from_min_size(
                egui::pos2((rect.right() - 16.0 - size.x).max(rect.left()), rect.top() + 8.0),
                size,
            );
            let palette = crate::theme::p();
            painter.rect_filled(bar, 6.0, palette.surface);
            painter.rect_stroke(bar, 6.0, Stroke::new(1.0, palette.border), StrokeKind::Inside);
            // Un clic sur le fond de la barre ne doit pas atteindre le terminal.
            ui.interact(bar, response.id.with("search_bar"), Sense::click());

            // +1 = vers le haut (plus ancien), -1 = vers le bas (plus récent).
            let mut step: i32 = 0;
            let mut changed = false;
            let mut field_focused = false;
            let margin = Vec2::new(6.0, 3.0);
            let builder = egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(bar.min + margin, Vec2::new(600.0, bar.height() - 2.0 * margin.y)))
                .layout(egui::Layout::left_to_right(egui::Align::Center));
            let content = ui.scope_builder(builder, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.spacing_mut().button_padding = Vec2::new(2.0, 2.0);
                // Champ façon VS Code : la saisie et les trois options dans un même cadre.
                let left = ui.cursor().min.x;
                let field = Rect::from_min_max(
                    egui::pos2(left, ui.max_rect().top()),
                    egui::pos2(left + field_width, ui.max_rect().bottom()),
                );
                ui.painter().rect_filled(field, 4.0, palette.bg);
                let options_width = 3.0 * (SEARCH_OPTION_SIZE.x + 1.0);
                ui.add_space(10.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut search.query)
                        .hint_text("Rechercher")
                        .frame(egui::Frame::NONE)
                        .align(egui::Align2::LEFT_CENTER)
                        .desired_width(field_width - options_width - 30.0),
                );
                // Les options sont calées contre le bord droit du champ.
                ui.add_space((field.right() - 3.0 - options_width - ui.cursor().min.x).max(0.0));
                if std::mem::take(&mut search.focus) {
                    edit.request_focus();
                }
                changed = edit.changed();
                // Options, à côté du champ de saisie.
                changed |= search_option(ui, &mut search.case_sensitive, "Aa", false, "Respecter la casse");
                changed |= search_option(ui, &mut search.whole_word, "ab", true, "Mot entier");
                changed |= search_option(ui, &mut search.regex, ".*", false, "Expression régulière");
                ui.add_space((field.right() - ui.cursor().min.x).max(0.0));
                let border = if edit.has_focus() { palette.accent } else { palette.border };
                ui.painter().rect_stroke(field, 4.0, Stroke::new(1.0, border), StrokeKind::Inside);
                let (enter, shift, escape) =
                    ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
                if edit.lost_focus() && enter {
                    step = if shift { -1 } else { 1 };
                    edit.request_focus();
                }
                if (edit.has_focus() || edit.lost_focus()) && escape {
                    close_search = true;
                }
                field_focused = edit.has_focus();

                // Compteur : position du résultat courant, comptée depuis le haut de l'historique.
                let has_results = !search.matches.is_empty();
                let (count, count_color) = if search.invalid {
                    ("Regex invalide".to_string(), palette.red)
                } else if has_results {
                    (format!("{} sur {}", search.current + 1, search.matches.len()), palette.text)
                } else {
                    ("Aucun résultat".to_string(), palette.text_dim)
                };
                ui.add_space(8.0);
                let (count_rect, _) =
                    ui.allocate_exact_size(Vec2::new(SEARCH_COUNT_WIDTH, ui.available_height()), Sense::hover());
                ui.painter().text(
                    count_rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    count,
                    FontId::proportional(11.0),
                    count_color,
                );
                // Flèches grisées tant qu'il n'y a pas de résultat.
                ui.add_enabled_ui(has_results, |ui| {
                    if crate::theme::header_button(ui, &crate::theme::ICON_ARROW_UP, "Résultat précédent (Entrée)")
                        .clicked()
                    {
                        step = 1;
                    }
                    if crate::theme::header_button(ui, &crate::theme::ICON_ARROW_DOWN, "Résultat suivant (Maj+Entrée)")
                        .clicked()
                    {
                        step = -1;
                    }
                });
                if crate::theme::header_button(ui, &crate::theme::ICON_CLOSE, "Fermer (Échap)").clicked() {
                    close_search = true;
                }
            });
            let width = (content.response.rect.width() + 2.0 * margin.x).ceil();
            if width != search.bar_width || field_width != search.field_width {
                search.bar_width = width;
                search.field_width = field_width;
                ui.ctx().request_repaint();
            }
            if field_focused {
                out.focused = true;
            }

            // Relecture de l'historique : texte modifié, navigation, ou délai écoulé.
            let key: SearchKey = (search.query.clone(), search.case_sensitive, search.whole_word, search.regex);
            let stale = match &search.scanned {
                Some((scanned, at)) => *scanned != key || at.elapsed() >= SEARCH_REFRESH,
                None => true,
            };
            if stale || step != 0 {
                (search.matches, search.invalid) =
                    match build_regex(&search.query, search.case_sensitive, search.whole_word, search.regex) {
                        None => (Vec::new(), false),
                        Some(Ok(pattern)) => (find_matches(&mut parser, &pattern), false),
                        Some(Err(_)) => (Vec::new(), true),
                    };
                search.scanned = Some((key, Instant::now()));
                // Nouvelle recherche : on part du résultat le plus récent (en bas).
                if changed || search.current >= search.matches.len() {
                    search.current = search.matches.len().saturating_sub(1);
                }
            }
            if step != 0 && !search.matches.is_empty() {
                let count = search.matches.len();
                search.current = if step > 0 {
                    (search.current + count - 1) % count
                } else {
                    (search.current + 1) % count
                };
            }
            if changed || step != 0 {
                if let Some(found) = search.matches.get(search.current) {
                    reveal(&mut parser, found.line);
                }
                self.selection = None;
                ui.ctx().request_repaint();
            }
        }
        if close_search {
            self.search = None;
            self.wants_focus = true;
            ui.ctx().request_repaint();
        }

        out
    }

    fn ordered_selection(&self) -> Option<(CellPos, CellPos)> {
        let (a, b) = self.selection?;
        if a == b {
            return None;
        }
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self, screen: &vt100::Screen) -> Option<String> {
        let (start, end) = self.ordered_selection()?;
        Some(screen.contents_between(start.0, start.1, end.0, end.1 + 1))
    }
}

fn nonzero(v: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(v.max(1)).expect("valeur non nulle")
}

/// Questions qu'un programme peut poser au terminal, et qui attendent une réponse.
const QUERIES: [(&[u8], Query); 4] = [
    (b"\x1b[6n", Query::CursorPosition),
    (b"\x1b[5n", Query::Status),
    (b"\x1b[c", Query::Attributes),
    (b"\x1b[0c", Query::Attributes),
];

#[derive(Clone, Copy)]
enum Query {
    CursorPosition,
    Status,
    Attributes,
}

/// Envoie la sortie d'un programme au terminal et renvoie les réponses aux
/// questions qu'elle contient (position du curseur, état, type de terminal).
///
/// Indispensable pour la console Windows (ConPTY), qui demande la position du
/// curseur au démarrage et attend la réponse avant d'afficher quoi que ce soit.
/// Chaque réponse reflète l'état du terminal au moment précis de la question.
pub fn feed(parser: &mut vt100::Parser, data: &[u8]) -> Vec<u8> {
    let mut replies = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1b {
            if let Some((pattern, query)) = QUERIES.iter().find(|(p, _)| data[i..].starts_with(p)) {
                let end = i + pattern.len();
                parser.process(&data[start..end]);
                start = end;
                i = end;
                match query {
                    Query::CursorPosition => {
                        let (row, col) = parser.screen().cursor_position();
                        replies.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                    }
                    Query::Status => replies.extend_from_slice(b"\x1b[0n"),
                    Query::Attributes => replies.extend_from_slice(b"\x1b[?1;2c"),
                }
                continue;
            }
        }
        i += 1;
    }
    parser.process(&data[start..]);
    replies
}

/// Nombre de lignes d'historique au-dessus de l'écran. vt100 ne le donne pas :
/// on le lit en demandant le décalage maximal, puis on remet le décalage courant.
fn history_len(parser: &mut vt100::Parser) -> usize {
    let offset = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(usize::MAX);
    let history = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(offset);
    history
}

/// Option de la barre de recherche, dessinée dans le champ de saisie : fond et
/// liseré quand elle est active. Renvoie vrai si elle vient de changer.
fn search_option(ui: &mut egui::Ui, value: &mut bool, text: &str, underline: bool, tooltip: &str) -> bool {
    let palette = crate::theme::p();
    ui.add_space(1.0);
    let (rect, response) = ui.allocate_exact_size(SEARCH_OPTION_SIZE, Sense::click());
    let clicked = response.clicked();
    if clicked {
        *value = !*value;
    }
    let hovered = response.hovered();
    response.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if *value {
            painter.rect_filled(rect, 3.0, palette.accent_soft);
            painter.rect_stroke(rect, 3.0, Stroke::new(1.0, palette.accent), StrokeKind::Inside);
        } else if hovered {
            painter.rect_filled(rect, 3.0, palette.surface_hover);
        }
        let color = if *value || hovered { palette.text } else { palette.text_dim };
        let drawn = painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, FontId::proportional(12.5), color);
        if underline {
            let y = drawn.bottom() + 0.5;
            painter.line_segment([egui::pos2(drawn.left(), y), egui::pos2(drawn.right(), y)], Stroke::new(1.0, color));
        }
    }
    clicked
}

/// Motif de recherche. `None` si le texte est vide, `Err` si l'expression
/// régulière saisie est invalide. Hors mode « expression régulière », le texte
/// est cherché tel quel.
fn build_regex(
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
) -> Option<Result<regex::Regex, regex::Error>> {
    if query.is_empty() {
        return None;
    }
    let mut pattern = if regex { query.to_string() } else { regex::escape(query) };
    if whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    Some(regex::RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).build())
}

/// Texte d'une ligne visible, sans les cellules vides de fin de ligne, et pour
/// chaque octet de ce texte la colonne et la largeur (en colonnes) de son caractère.
fn line_text(screen: &vt100::Screen, row: u16, cols: u16) -> (String, Vec<(u16, u16)>) {
    let mut text = String::new();
    let mut used = 0;
    let mut columns = Vec::new();
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else { break };
        if cell.is_wide_continuation() {
            continue;
        }
        let width = if cell.is_wide() { 2 } else { 1 };
        let contents = if cell.has_contents() { cell.contents() } else { " " };
        text.push_str(contents);
        columns.extend(std::iter::repeat_n((col, width), contents.len()));
        if cell.has_contents() {
            used = text.len();
        }
    }
    text.truncate(used);
    columns.truncate(used);
    (text, columns)
}

/// Occurrences (sans chevauchement) du motif dans une ligne : première et
/// dernière colonnes de chacune. Les occurrences vides sont ignorées.
fn find_in_line(text: &str, columns: &[(u16, u16)], pattern: &regex::Regex) -> Vec<(u16, u16)> {
    pattern
        .find_iter(text)
        .filter(|found| !found.is_empty())
        .map(|found| {
            let (last, width) = columns[found.end() - 1];
            (columns[found.start()].0, last + width - 1)
        })
        .collect()
}

/// Cherche le motif dans tout l'historique et l'écran, de la ligne la plus
/// ancienne à la plus récente. Une occurrence coupée par un retour à la ligne
/// automatique n'est pas trouvée.
fn find_matches(parser: &mut vt100::Parser, pattern: &regex::Regex) -> Vec<Match> {
    let mut matches = Vec::new();
    let (rows, cols) = parser.screen().size();
    let (rows, cols) = (rows.get(), cols.get());
    let saved = parser.screen().scrollback();
    let history = history_len(parser);

    // L'écran ne montre que `rows` lignes à la fois : on le fait défiler du
    // haut de l'historique jusqu'en bas, sans relire deux fois la même ligne.
    let mut next = 0; // prochaine ligne à lire
    let mut offset = history;
    loop {
        parser.screen_mut().set_scrollback(offset);
        let first = history - offset;
        let screen = parser.screen();
        for row in 0..rows {
            let line = first + row as usize;
            if line < next {
                continue;
            }
            let (text, columns) = line_text(screen, row, cols);
            for (start, end) in find_in_line(&text, &columns, pattern) {
                matches.push(Match { line, start, end });
            }
        }
        next = first + rows as usize;
        if offset == 0 {
            break;
        }
        offset = offset.saturating_sub(rows as usize);
    }
    parser.screen_mut().set_scrollback(saved);
    matches
}

/// Fait défiler l'historique pour afficher la ligne `line` (comptée depuis le
/// haut de l'historique), au milieu de l'écran si elle n'est pas déjà visible.
fn reveal(parser: &mut vt100::Parser, line: usize) {
    let rows = parser.screen().size().0.get() as usize;
    let history = history_len(parser);
    let first = history - parser.screen().scrollback().min(history);
    if line >= first && line < first + rows {
        return;
    }
    let target = line.saturating_sub(rows / 2).min(history);
    parser.screen_mut().set_scrollback(history - target);
}

fn scroll_history(parser: &mut vt100::Parser, delta: i64) {
    let current = parser.screen().scrollback() as i64;
    parser.screen_mut().set_scrollback((current + delta).max(0) as usize);
}

fn paste(out: &mut Vec<u8>, text: &str, bracketed: bool) {
    // Les terminaux envoient un retour chariot pour chaque fin de ligne.
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
    }
    out.extend_from_slice(text.as_bytes());
    if bracketed {
        out.extend_from_slice(b"\x1b[201~");
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: Color32,
    bg: Color32,
    italic: bool,
    underline: bool,
}

impl CellStyle {
    fn of(cell: &vt100::Cell) -> Self {
        let mut fg = match cell.fgcolor() {
            // Le gras éclaircit les 8 couleurs de base, comme la plupart des terminaux.
            vt100::Color::Idx(i) if cell.bold() && i < 8 => indexed(i + 8),
            c => color(c, default_fg()),
        };
        let mut bg = color(cell.bgcolor(), default_bg());
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.dim() {
            fg = blend(fg, bg);
        }
        Self {
            fg,
            bg,
            italic: cell.italic(),
            underline: cell.underline(),
        }
    }
}

/// Couleur des mots-clés de la ligne `row`, colonne par colonne.
fn row_keywords(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    highlighter: &crate::highlight::Highlighter,
) -> Vec<Option<Color32>> {
    let mut colors = vec![None; cols as usize];
    // Texte de la ligne, et colonne de début de chaque caractère (en octets).
    let mut text = String::new();
    let mut starts: Vec<(usize, u16, bool)> = Vec::with_capacity(cols as usize);
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else { break };
        if cell.is_wide_continuation() {
            continue;
        }
        starts.push((text.len(), col, cell.is_wide()));
        if cell.has_contents() {
            text.push_str(cell.contents());
        } else {
            text.push(' ');
        }
    }
    if text.trim().is_empty() {
        return colors;
    }
    for (range, color) in highlighter.find(&text) {
        let color = color.color();
        for &(start, col, wide) in &starts {
            if range.contains(&start) {
                colors[col as usize] = Some(color);
                if wide && col + 1 < cols {
                    colors[col as usize + 1] = Some(color);
                }
            }
        }
    }
    colors
}

fn color(c: vt100::Color, default: Color32) -> Color32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => indexed(i),
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

/// Palette 256 couleurs : 16 de base, cube 6×6×6, puis 24 niveaux de gris.
fn indexed(i: u8) -> Color32 {
    match i {
        0..=15 => crate::theme::p().ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

fn blend(a: Color32, b: Color32) -> Color32 {
    let mix = |x: u8, y: u8| ((x as u16 * 3 + y as u16 * 2) / 5) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Traduit une touche en séquence xterm.
fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    let modcode = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let cursor = |c: char| -> Vec<u8> {
        if modcode > 1 {
            format!("\x1b[1;{modcode}{c}")
        } else if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modcode > 1 {
            format!("\x1b[{n};{modcode}~")
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes()
    };
    Some(match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if m.ctrl => vec![0x08],
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        // Ctrl+lettre. AltGr arrive comme Alt seul (sans Ctrl), il n'est donc pas
        // intercepté ici et produit bien son caractère (@, #, €…).
        _ if m.ctrl && !m.alt => return ctrl_byte(key).map(|b| vec![b]),
        _ => return None,
    })
}

fn ctrl_byte(key: Key) -> Option<u8> {
    let name = key.name().as_bytes();
    if name.len() == 1 && name[0].is_ascii_uppercase() {
        return Some(name[0] - b'A' + 1);
    }
    match key {
        Key::Space => Some(0x00),
        Key::OpenBracket => Some(0x1b),
        Key::Backslash => Some(0x1c),
        Key::CloseBracket => Some(0x1d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        let none = Modifiers::default();
        assert_eq!(key_bytes(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(key_bytes(Key::ArrowLeft, ctrl, true).unwrap(), b"\x1b[1;5D");
    }

    #[test]
    fn ctrl_letters_but_not_altgr() {
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        assert_eq!(key_bytes(Key::D, ctrl, false).unwrap(), vec![0x04]);
        let altgr_like = Modifiers { ctrl: true, alt: true, ..Default::default() };
        assert_eq!(key_bytes(Key::E, altgr_like, false), None);
        assert_eq!(key_bytes(Key::E, Modifiers::default(), false), None);
    }

    #[test]
    fn bracketed_paste_wraps_and_converts_newlines() {
        let mut out = Vec::new();
        paste(&mut out, "a\r\nb\n", true);
        assert_eq!(out, b"\x1b[200~a\rb\r\x1b[201~");
    }

    /// La mini barre de défilement lit la longueur de l'historique en
    /// demandant le décalage maximal : vt100 doit le borner sans paniquer.
    #[test]
    fn max_scrollback_gives_history_length() {
        let mut parser = vt100::Parser::new(nonzero(5), nonzero(20), 1000);
        for line in 0..12 {
            parser.process(format!("ligne {line}\r\n").as_bytes());
        }
        parser.screen_mut().set_scrollback(usize::MAX);
        // 12 lignes + la ligne courante sur 5 lignes d'écran : 8 lignes d'historique.
        assert_eq!(parser.screen().scrollback(), 8);
        parser.screen_mut().set_scrollback(0);
        assert_eq!(parser.screen().scrollback(), 0);
    }

    #[test]
    fn answers_cursor_position_at_query_time() {
        let mut parser = vt100::Parser::new(nonzero(24), nonzero(80), 0);
        // Le curseur est en ligne 2, colonne 4 au moment de la question,
        // même si d'autres caractères suivent dans le même bloc.
        let replies = feed(&mut parser, b"ab\r\nxyz\x1b[6nSUITE\x1b[c");
        assert_eq!(replies, b"\x1b[2;4R\x1b[?1;2c");
        assert_eq!(parser.screen().contents(), "ab\nxyzSUITE");
    }

    fn parser_with(rows: u16, cols: u16, text: &str) -> vt100::Parser {
        let mut parser = vt100::Parser::new(nonzero(rows), nonzero(cols), 100);
        parser.process(text.as_bytes());
        parser
    }

    /// Recherche avec des options données : (casse, mot entier, expression régulière).
    fn search(parser: &mut vt100::Parser, query: &str, options: (bool, bool, bool)) -> Vec<Match> {
        let pattern = build_regex(query, options.0, options.1, options.2).expect("texte non vide").expect("motif");
        find_matches(parser, &pattern)
    }

    /// Recherche simple : texte tel quel, sans tenir compte de la casse.
    fn plain(parser: &mut vt100::Parser, query: &str) -> Vec<Match> {
        search(parser, query, (false, false, false))
    }

    #[test]
    fn search_options_case_word_and_regex() {
        let mut parser = parser_with(3, 40, "Error errors ERROR a.b axb");
        let columns = |found: Vec<Match>| found.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>();
        // Casse ignorée par défaut, respectée sur demande.
        assert_eq!(plain(&mut parser, "error").len(), 3);
        assert_eq!(columns(search(&mut parser, "error", (true, false, false))), vec![(6, 10)]);
        // Mot entier : « errors » ne compte pas.
        assert_eq!(columns(search(&mut parser, "error", (false, true, false))), vec![(0, 4), (13, 17)]);
        // Texte simple : le point est un point ; en expression régulière, un caractère quelconque.
        assert_eq!(columns(plain(&mut parser, "a.b")), vec![(19, 21)]);
        assert_eq!(columns(search(&mut parser, "a.b", (false, false, true))), vec![(19, 21), (23, 25)]);
        assert_eq!(columns(search(&mut parser, r"err\w+s", (true, false, true))), vec![(6, 11)]);
        // Expression invalide signalée, texte vide sans motif, occurrences vides ignorées.
        assert!(build_regex("(", false, false, true).expect("texte non vide").is_err());
        assert!(build_regex("(", false, false, false).expect("texte non vide").is_ok());
        assert!(build_regex("", false, false, false).is_none());
        assert!(search(&mut parser, "z*", (false, false, true)).is_empty());
    }

    #[test]
    fn search_finds_matches_in_history_and_screen() {
        // 10 lignes sur un écran de 4 : plusieurs passent dans l'historique.
        let text: String = (0..10).map(|i| format!("ligne {i} Erreur\r\n")).collect();
        let mut parser = parser_with(4, 30, &text);
        let found = plain(&mut parser, "erreur");
        // Chaque ligne n'est lue qu'une fois, de la plus ancienne à la plus récente.
        assert_eq!(found.len(), 10);
        assert_eq!(found[0], Match { line: 0, start: 8, end: 13 });
        assert_eq!(found[9].line, 9);
        // Le défilement de l'utilisateur est restauré.
        assert_eq!(parser.screen().scrollback(), 0);
        assert_eq!(plain(&mut parser, "ligne 7").len(), 1);
        assert!(plain(&mut parser, "absent").is_empty());
    }

    #[test]
    fn search_handles_repeats_and_wide_chars() {
        let mut parser = parser_with(3, 20, "aaaa 日本 ab");
        let found = plain(&mut parser, "aa");
        assert_eq!(found.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>(), vec![(0, 1), (2, 3)]);
        // Un caractère large occupe deux colonnes.
        let wide = plain(&mut parser, "日本");
        assert_eq!((wide[0].start, wide[0].end), (5, 8));
    }

    #[test]
    fn reveal_scrolls_only_when_needed() {
        let text: String = (0..30).map(|i| format!("l{i}\r\n")).collect();
        let mut parser = parser_with(5, 20, &text);
        let history = history_len(&mut parser);
        // Ligne déjà visible : pas de défilement.
        reveal(&mut parser, history + 1);
        assert_eq!(parser.screen().scrollback(), 0);
        // Ligne ancienne : elle devient visible.
        reveal(&mut parser, 3);
        let first = history - parser.screen().scrollback();
        assert!((first..first + 5).contains(&3));
        assert_eq!(parser.screen().contents_between((3 - first) as u16, 0, (3 - first) as u16, 2), "l3");
    }

    #[test]
    fn plain_output_has_no_reply() {
        let mut parser = vt100::Parser::new(nonzero(24), nonzero(80), 0);
        assert!(feed(&mut parser, b"\x1b[31mrouge\x1b[0m").is_empty());
    }

    #[test]
    fn keywords_are_mapped_to_columns() {
        let highlighter = crate::highlight::Highlighter::new(&crate::highlight::Highlighting::default()).unwrap();
        let mut parser = vt100::Parser::new(nonzero(2), nonzero(30), 0);
        // « 漢 » occupe deux colonnes : les mots qui suivent sont décalés d'autant.
        parser.process("漢 ok, error!".as_bytes());
        let colors = row_keywords(parser.screen(), 0, 30, &highlighter);
        let colored: Vec<usize> = (0..30).filter(|&c| colors[c].is_some()).collect();
        assert_eq!(colored, vec![3, 4, 7, 8, 9, 10, 11]);
        let green = crate::highlight::HighlightColor::Green.color();
        let red = crate::highlight::HighlightColor::Red.color();
        assert_eq!((colors[3], colors[7]), (Some(green), Some(red)));
    }

    #[test]
    fn palette_256() {
        assert_eq!(indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(indexed(232), Color32::from_rgb(8, 8, 8));
    }
}
