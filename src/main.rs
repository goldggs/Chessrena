use chess::{Board, Color, File, Piece, Rank, Square};
use eframe::egui;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::PathBuf,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
struct MatchRecord {
    id: i64,
    name: String,
    white_name: String,
    black_name: String,
    status: String,
    fen: String,
    moves: String,
}

struct Store {
    db: Connection,
}

impl Store {
    fn open(path: PathBuf) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(db: Connection) -> rusqlite::Result<Self> {
        db.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS matches (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                 white_name TEXT NOT NULL DEFAULT 'Waiting for agent',
                 black_name TEXT NOT NULL DEFAULT 'Waiting for agent',
                 status TEXT NOT NULL DEFAULT 'Waiting',
                 fen TEXT NOT NULL,
                 moves TEXT NOT NULL DEFAULT '',
                 updated_at INTEGER NOT NULL
             );",
        )?;
        Ok(Self { db })
    }

    fn list(&self) -> rusqlite::Result<Vec<MatchRecord>> {
        let mut stmt = self.db.prepare(
            "SELECT id, name, white_name, black_name, status, fen, moves, updated_at
             FROM matches ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MatchRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                white_name: row.get(2)?,
                black_name: row.get(3)?,
                status: row.get(4)?,
                fen: row.get(5)?,
                moves: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    fn get(&self, id: i64) -> rusqlite::Result<Option<MatchRecord>> {
        self.db.query_row(
            "SELECT id, name, white_name, black_name, status, fen, moves, updated_at FROM matches WHERE id = ?1",
            [id],
            |row| Ok(MatchRecord {
                id: row.get(0)?, name: row.get(1)?, white_name: row.get(2)?, black_name: row.get(3)?,
                status: row.get(4)?, fen: row.get(5)?, moves: row.get(6)?,
            }),
        ).optional()
    }

    fn create(&self) -> rusqlite::Result<i64> {
        let mut n = 1;
        loop {
            let name = format!("New match {n}");
            let exists: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM matches WHERE name = ?1 COLLATE NOCASE)",
                [&name],
                |r| r.get(0),
            )?;
            if !exists {
                self.db.execute(
                    "INSERT INTO matches(name, status, fen, updated_at) VALUES(?1, 'Waiting', ?2, ?3)",
                    params![name, Board::default().to_string(), now()],
                )?;
                return Ok(self.db.last_insert_rowid());
            }
            n += 1;
        }
    }

    fn rename(&self, id: i64, name: &str) -> rusqlite::Result<()> {
        let clean = name.trim();
        if clean.is_empty() || clean.chars().count() > 48 {
            return Err(rusqlite::Error::InvalidParameterName(
                "Match name must be 1 to 48 characters".into(),
            ));
        }
        self.db.execute(
            "UPDATE matches SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![clean, now(), id],
        )?;
        Ok(())
    }

    fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM matches WHERE id = ?1", [id])?;
        Ok(())
    }

    fn close_match(&self, id: i64) -> rusqlite::Result<()> {
        self.db.execute("UPDATE matches SET status = 'Paused', updated_at = ?1 WHERE id = ?2 AND status = 'In progress'", params![now(), id])?;
        Ok(())
    }

    fn set_status(&self, id: i64, status: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE matches SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now(), id],
        )?;
        Ok(())
    }
}

struct ChessrenaApp {
    store: Store,
    selected: Option<i64>,
    rename_target: Option<(i64, String)>,
    delete_target: Option<i64>,
    error: Option<String>,
}

impl ChessrenaApp {
    fn new() -> Result<Self, String> {
        let path = data_dir()?.join("chessrena.db");
        let store =
            Store::open(path).map_err(|e| format!("Could not open local match database: {e}"))?;
        Ok(Self {
            store,
            selected: None,
            rename_target: None,
            delete_target: None,
            error: None,
        })
    }

    fn show_board(ui: &mut egui::Ui, fen: &str, size: f32) {
        let Ok(board) = Board::from_str(fen) else {
            ui.colored_label(egui::Color32::LIGHT_RED, "Position data could not be read.");
            return;
        };
        egui::Grid::new("chessboard")
            .spacing(egui::vec2(0.0, 0.0))
            .show(ui, |ui| {
                for r in 0..8 {
                    for f in 0..8 {
                        let square =
                            Square::make_square(Rank::from_index(7 - r), File::from_index(f));
                        let light = (r + f) % 2 == 0;
                        let color = if light {
                            egui::Color32::from_rgb(216, 211, 196)
                        } else {
                            egui::Color32::from_rgb(99, 118, 106)
                        };
                        let piece = board
                            .piece_on(square)
                            .map(|p| piece_glyph(p, board.color_on(square).unwrap_or(Color::White)))
                            .unwrap_or(' ');
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(size / 8.0, size / 8.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 0.0, color);
                        if piece != ' ' {
                            let ink = if board.color_on(square) == Some(Color::White) {
                                egui::Color32::from_rgb(250, 249, 242)
                            } else {
                                egui::Color32::from_rgb(31, 39, 33)
                            };
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                piece,
                                egui::FontId::proportional(size / 12.0),
                                ink,
                            );
                        }
                    }
                    ui.end_row();
                }
            });
    }
}

impl eframe::App for ChessrenaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let bg = egui::Color32::from_rgb(17, 20, 18);
        let surface = egui::Color32::from_rgb(25, 29, 26);
        let border = egui::Color32::from_rgb(52, 59, 54);
        let accent = egui::Color32::from_rgb(168, 213, 177);
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = bg;
        style.visuals.window_fill = surface;
        style.visuals.widgets.noninteractive.bg_fill = surface;
        style.visuals.widgets.noninteractive.bg_stroke.color = border;
        style.visuals.widgets.inactive.bg_fill = surface;
        style.visuals.widgets.inactive.bg_stroke.color = border;
        style.visuals.selection.bg_fill = accent;
        style.visuals.selection.stroke.color = egui::Color32::from_rgb(23, 34, 26);
        ctx.set_style(style);

        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(21, 25, 22))
                    .inner_margin(egui::Margin::symmetric(22, 13)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("C").color(accent).strong().size(18.0));
                    ui.label(egui::RichText::new("Chessrena").strong().size(16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Local app")
                                .color(egui::Color32::GRAY)
                                .small(),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(bg)
                    .inner_margin(egui::Margin::symmetric(28, 25)),
            )
            .show(ctx, |ui| {
                if let Some(message) = self.error.take() {
                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                    ui.add_space(8.0);
                }
                if let Some(id) = self.selected {
                    self.show_detail(ui, id, accent, surface, border);
                } else {
                    self.show_home(ui, accent, surface, border);
                }
            });
        self.show_dialogs(ctx, surface, border);
    }
}

impl ChessrenaApp {
    fn show_home(
        &mut self,
        ui: &mut egui::Ui,
        accent: egui::Color32,
        surface: egui::Color32,
        border: egui::Color32,
    ) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Matches");
                ui.label(
                    egui::RichText::new("Open a saved match or create a new one.")
                        .color(egui::Color32::GRAY),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("+  New match")
                                .color(egui::Color32::from_rgb(23, 34, 26))
                                .strong(),
                        )
                        .fill(accent),
                    )
                    .clicked()
                {
                    match self.store.create() {
                        Ok(id) => self.selected = Some(id),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            });
        });
        ui.add_space(22.0);
        ui.label(
            egui::RichText::new("SAVED MATCHES")
                .small()
                .color(egui::Color32::GRAY)
                .strong(),
        );
        ui.add_space(9.0);
        match self.store.list() {
            Ok(matches) if matches.is_empty() => {
                egui::Frame::group(ui.style())
                    .fill(surface)
                    .stroke(egui::Stroke::new(1.0_f32, border))
                    .show(ui, |ui| {
                        ui.set_min_height(110.0);
                        ui.centered_and_justified(|ui| {
                            ui.label("No matches yet. Create a match to get started.");
                        });
                    });
            }
            Ok(matches) => {
                let count = matches.len();
                egui::Grid::new("match_cards")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 12.0))
                    .show(ui, |ui| {
                        for (index, m) in matches.into_iter().enumerate() {
                            egui::Frame::group(ui.style())
                                .fill(surface)
                                .stroke(egui::Stroke::new(1.0_f32, border))
                                .inner_margin(12.0)
                                .show(ui, |ui| {
                                    ui.set_min_width(390.0);
                                    ui.horizontal(|ui| {
                                        Self::show_board(ui, &m.fen, 90.0);
                                        ui.add_space(10.0);
                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .selectable_label(
                                                        false,
                                                        egui::RichText::new(&m.name).strong(),
                                                    )
                                                    .clicked()
                                                {
                                                    self.selected = Some(m.id);
                                                }
                                                ui.menu_button("⋯", |ui| {
                                                    if ui.button("Rename").clicked() {
                                                        self.rename_target =
                                                            Some((m.id, m.name.clone()));
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("Delete match").clicked() {
                                                        self.delete_target = Some(m.id);
                                                        ui.close_menu();
                                                    }
                                                });
                                            });
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "White {}   Black {}",
                                                    m.white_name, m.black_name
                                                ))
                                                .color(egui::Color32::GRAY)
                                                .small(),
                                            );
                                            ui.add_space(8.0);
                                            let status_color = if m.status == "Finished" {
                                                egui::Color32::GRAY
                                            } else {
                                                accent
                                            };
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "● {}   {}",
                                                    m.status,
                                                    last_move(&m.moves)
                                                ))
                                                .color(status_color)
                                                .small(),
                                            );
                                            if ui
                                                .add(
                                                    egui::Button::new("Open match")
                                                        .stroke(egui::Stroke::new(1.0_f32, border)),
                                                )
                                                .clicked()
                                            {
                                                self.selected = Some(m.id);
                                            }
                                        });
                                    });
                                });
                            if index % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        if count % 2 == 1 {
                            ui.end_row();
                        }
                    });
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn show_detail(
        &mut self,
        ui: &mut egui::Ui,
        id: i64,
        accent: egui::Color32,
        surface: egui::Color32,
        border: egui::Color32,
    ) {
        let record = match self.store.get(id) {
            Ok(Some(m)) => m,
            Ok(None) => {
                self.selected = None;
                return;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        ui.horizontal(|ui| {
            if ui.button("‹  Matches").clicked() {
                if let Err(e) = self.store.close_match(id) {
                    self.error = Some(e.to_string());
                }
                self.selected = None;
            }
            ui.add_space(8.0);
            ui.heading(&record.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if record.status != "Finished"
                    && ui
                        .button(
                            egui::RichText::new("End match")
                                .color(egui::Color32::from_rgb(228, 154, 145)),
                        )
                        .clicked()
                {
                    if let Err(e) = self.store.set_status(id, "Finished") {
                        self.error = Some(e.to_string());
                    }
                }
            });
        });
        ui.add_space(16.0);
        ui.columns(2, |cols| {
            cols[0].horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(format!("{} to move", side_to_move(&record.fen))).strong());
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style()).fill(surface).stroke(egui::Stroke::new(1.0_f32, border)).inner_margin(12.0).show(ui, |ui| Self::show_board(ui, &record.fen, 520.0));
                });
            });
            cols[1].vertical(|ui| {
                egui::Frame::group(ui.style()).fill(surface).stroke(egui::Stroke::new(1.0_f32, border)).inner_margin(14.0).show(ui, |ui| {
                    ui.label(egui::RichText::new("AGENTS").small().strong().color(egui::Color32::GRAY));
                    ui.add_space(11.0);
                    ui.label(format!("White  {}", record.white_name));
                    ui.separator();
                    ui.label(format!("Black  {}", record.black_name));
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new("Agent registration and live match controls are the next implementation step.").small().color(egui::Color32::GRAY));
                });
                ui.add_space(12.0);
                egui::Frame::group(ui.style()).fill(surface).stroke(egui::Stroke::new(1.0_f32, border)).inner_margin(14.0).show(ui, |ui| {
                    ui.label(egui::RichText::new("MOVE HISTORY").small().strong().color(egui::Color32::GRAY));
                    ui.add_space(8.0);
                    ui.label(if record.moves.is_empty() { "No moves yet".to_owned() } else { record.moves.replace(',', "  ") });
                });
                ui.add_space(12.0);
                if record.status == "Paused" || record.status == "Waiting" {
                    ui.add_enabled(false, egui::Button::new("Start match").fill(accent));
                    ui.label(egui::RichText::new("Start will be enabled after both agents are ready.").small().color(egui::Color32::GRAY));
                }
            });
        });
    }

    fn show_dialogs(&mut self, ctx: &egui::Context, surface: egui::Color32, border: egui::Color32) {
        if let Some((id, mut name)) = self.rename_target.take() {
            let mut open = true;
            let mut save = false;
            egui::Window::new("Rename match")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Match names must be unique.");
                    ui.text_edit_singleline(&mut name);
                    if ui.button("Save name").clicked() {
                        save = true;
                    }
                });
            if open && !save {
                self.rename_target = Some((id, name.clone()));
            }
            if save {
                if let Err(e) = self.store.rename(id, &name) {
                    self.error = Some(format!("Could not rename match: {e}"));
                }
            }
        }
        if let Some(id) = self.delete_target {
            let mut open = true;
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete match")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Delete this match and its saved game data? This cannot be undone.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if ui
                            .button(
                                egui::RichText::new("Delete match")
                                    .color(egui::Color32::from_rgb(228, 154, 145)),
                            )
                            .clicked()
                        {
                            confirm = true;
                        }
                    });
                });
            if !open || cancel || confirm {
                self.delete_target = None;
            }
            if confirm {
                if let Err(e) = self.store.delete(id) {
                    self.error = Some(e.to_string());
                }
            }
        }
        let _ = (surface, border);
    }
}

fn data_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("Chessrena"))
            .ok_or_else(|| "LOCALAPPDATA is not set".to_owned())
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(path).join("chessrena"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_owned())?;
        Ok(PathBuf::from(home).join(".local/share/chessrena"))
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn last_move(moves: &str) -> &str {
    moves
        .split(',')
        .rfind(|x| !x.is_empty())
        .unwrap_or("No moves")
}
fn side_to_move(fen: &str) -> &'static str {
    if fen.split_whitespace().nth(1) == Some("b") {
        "Black"
    } else {
        "White"
    }
}
fn piece_glyph(piece: Piece, color: Color) -> char {
    match (color, piece) {
        (Color::White, Piece::King) => '♔',
        (Color::White, Piece::Queen) => '♕',
        (Color::White, Piece::Rook) => '♖',
        (Color::White, Piece::Bishop) => '♗',
        (Color::White, Piece::Knight) => '♘',
        (Color::White, Piece::Pawn) => '♙',
        (Color::Black, Piece::King) => '♚',
        (Color::Black, Piece::Queen) => '♛',
        (Color::Black, Piece::Rook) => '♜',
        (Color::Black, Piece::Bishop) => '♝',
        (Color::Black, Piece::Knight) => '♞',
        (Color::Black, Piece::Pawn) => '♟',
    }
}

fn main() -> eframe::Result<()> {
    let app = ChessrenaApp::new().unwrap_or_else(|message| panic!("{message}"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("Chessrena", options, Box::new(|_cc| Ok(Box::new(app))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess::MoveGen;

    fn memory_store() -> Store {
        Store::from_connection(Connection::open_in_memory().expect("in-memory SQLite should open"))
            .expect("schema should initialize")
    }

    #[test]
    fn generated_match_names_are_unique() {
        let store = memory_store();
        let first = store.create().expect("first match should be created");
        let second = store.create().expect("second match should be created");
        assert_eq!(store.get(first).unwrap().unwrap().name, "New match 1");
        assert_eq!(store.get(second).unwrap().unwrap().name, "New match 2");
    }

    #[test]
    fn match_names_are_unique_without_case_sensitivity() {
        let store = memory_store();
        let first = store.create().unwrap();
        let second = store.create().unwrap();
        store.rename(first, "Opening test").unwrap();
        assert!(store.rename(second, " opening TEST ").is_err());
        assert_eq!(store.get(second).unwrap().unwrap().name, "New match 2");
    }

    #[test]
    fn closing_a_running_match_saves_it_as_paused() {
        let store = memory_store();
        let id = store.create().unwrap();
        store.set_status(id, "In progress").unwrap();
        store.close_match(id).unwrap();
        let saved = store.get(id).unwrap().unwrap();
        assert_eq!(saved.status, "Paused");
        assert_eq!(saved.fen, Board::default().to_string());
    }

    #[test]
    fn initial_chess_position_has_twenty_legal_moves() {
        assert_eq!(MoveGen::new_legal(&Board::default()).count(), 20);
    }
}
