mod api;

use chess::{Board, Color, File, Piece, Rank, Square};
use chessrena::game::{first_seat_gets_white, Game, Outcome, PlayedMove};
use eframe::egui;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub(crate) struct MatchRecord {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) white_name: String,
    pub(crate) black_name: String,
    pub(crate) status: String,
    pub(crate) fen: String,
    pub(crate) moves: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentSeat {
    pub(crate) seat: u8,
    pub(crate) name: Option<String>,
    pub(crate) registered: bool,
    pub(crate) ready: bool,
    pub(crate) color: Option<String>,
}

pub(crate) struct CreatedMatch {
    pub(crate) id: i64,
    pub(crate) first_invite: String,
    pub(crate) second_invite: String,
}

pub(crate) struct Store {
    db: Connection,
}

impl Store {
    pub(crate) fn open(path: PathBuf) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::from_connection(db)
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
             );
             CREATE TABLE IF NOT EXISTS agent_seats (
                 match_id INTEGER NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
                 seat INTEGER NOT NULL CHECK(seat IN (1, 2)),
                 token_hash TEXT NOT NULL,
                 agent_name TEXT,
                 registered INTEGER NOT NULL DEFAULT 0,
                 ready INTEGER NOT NULL DEFAULT 0,
                 color TEXT,
                 PRIMARY KEY(match_id, seat),
                 UNIQUE(match_id, token_hash)
             );",
        )?;
        Ok(Self { db })
    }

    pub(crate) fn list(&self) -> rusqlite::Result<Vec<MatchRecord>> {
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

    pub(crate) fn get(&self, id: i64) -> rusqlite::Result<Option<MatchRecord>> {
        self.db.query_row(
            "SELECT id, name, white_name, black_name, status, fen, moves, updated_at FROM matches WHERE id = ?1",
            [id],
            |row| Ok(MatchRecord {
                id: row.get(0)?, name: row.get(1)?, white_name: row.get(2)?, black_name: row.get(3)?,
                status: row.get(4)?, fen: row.get(5)?, moves: row.get(6)?,
            }),
        ).optional()
    }

    pub(crate) fn seat_for_token(&self, match_id: i64, token: &str) -> Result<AgentSeat, String> {
        let hashed = hash_token(token);
        self.db.query_row(
            "SELECT seat, agent_name, registered, ready, color FROM agent_seats WHERE match_id = ?1 AND token_hash = ?2",
            params![match_id, hashed],
            |row| Ok(AgentSeat {
                seat: row.get(0)?, name: row.get(1)?, registered: row.get::<_, i64>(2)? != 0,
                ready: row.get::<_, i64>(3)? != 0, color: row.get(4)?,
            }),
        ).map_err(|_| "Invalid match invitation".to_owned())
    }

    pub(crate) fn register_agent(
        &self,
        match_id: i64,
        token: &str,
        name: &str,
    ) -> Result<AgentSeat, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
            return Err("Agent name must be 1 to 40 visible characters".to_owned());
        }
        let seat = self.seat_for_token(match_id, token)?;
        let current = self
            .get(match_id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        if current.status == "Finished" || current.status == "In progress" {
            return Err(
                "Agents can only register before play or while a match is paused".to_owned(),
            );
        }
        let other_names: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT agent_name FROM agent_seats WHERE match_id = ?1 AND seat != ?2 AND agent_name IS NOT NULL").map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![match_id, seat.seat], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        if other_names
            .iter()
            .any(|other| other.trim().to_lowercase() == name.to_lowercase())
        {
            return Err("That agent name is already used in this match".to_owned());
        }
        self.db.execute("UPDATE agent_seats SET agent_name = ?1, registered = 1, ready = 0 WHERE match_id = ?2 AND seat = ?3", params![name, match_id, seat.seat]).map_err(|e| e.to_string())?;
        self.seat_for_token(match_id, token)
    }

    pub(crate) fn mark_agent_ready(&self, match_id: i64, token: &str) -> Result<AgentSeat, String> {
        let seat = self.seat_for_token(match_id, token)?;
        if !seat.registered {
            return Err("Register an agent name before marking ready".to_owned());
        }
        let current = self
            .get(match_id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        if current.status != "Waiting" && current.status != "Paused" {
            return Err("This match is not waiting for ready agents".to_owned());
        }
        self.db
            .execute(
                "UPDATE agent_seats SET ready = 1 WHERE match_id = ?1 AND seat = ?2",
                params![match_id, seat.seat],
            )
            .map_err(|e| e.to_string())?;
        self.seat_for_token(match_id, token)
    }

    pub(crate) fn seats(&self, match_id: i64) -> rusqlite::Result<Vec<AgentSeat>> {
        let mut stmt = self.db.prepare("SELECT seat, agent_name, registered, ready, color FROM agent_seats WHERE match_id = ?1 ORDER BY seat")?;
        let rows = stmt.query_map([match_id], |row| {
            Ok(AgentSeat {
                seat: row.get(0)?,
                name: row.get(1)?,
                registered: row.get::<_, i64>(2)? != 0,
                ready: row.get::<_, i64>(3)? != 0,
                color: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn start_match(&self, match_id: i64) -> Result<(), String> {
        let record = self
            .get(match_id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        if record.status != "Waiting" && record.status != "Paused" {
            return Err("This match cannot be started from its current state".to_owned());
        }
        let seats = self.seats(match_id).map_err(|e| e.to_string())?;
        if seats.len() != 2 || seats.iter().any(|s| !s.registered || !s.ready) {
            return Err("Both agents must register and mark ready first".to_owned());
        }
        let active: i64 = self
            .db
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE status = 'In progress' AND id != ?1",
                [match_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if active > 0 {
            return Err("Only one match can run at a time".to_owned());
        }
        if seats.iter().any(|seat| seat.color.is_none()) {
            let first_is_white = first_seat_gets_white().map_err(|e| e.to_string())?;
            let (first_color, second_color) = if first_is_white {
                ("white", "black")
            } else {
                ("black", "white")
            };
            self.db.execute("UPDATE agent_seats SET color = CASE seat WHEN 1 THEN ?1 ELSE ?2 END WHERE match_id = ?3", params![first_color, second_color, match_id]).map_err(|e| e.to_string())?;
        }
        self.db.execute("UPDATE matches SET status = 'In progress', white_name = (SELECT agent_name FROM agent_seats WHERE match_id = ?1 AND color = 'white'), black_name = (SELECT agent_name FROM agent_seats WHERE match_id = ?1 AND color = 'black'), updated_at = ?2 WHERE id = ?1", params![match_id, now()]).map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(test)]
    fn create(&self) -> rusqlite::Result<i64> {
        self.create_with_invites().map(|created| created.id)
    }

    pub(crate) fn create_with_invites(&self) -> rusqlite::Result<CreatedMatch> {
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
                let id = self.db.last_insert_rowid();
                let (first_invite, second_invite) = self.mint_invites(id)?;
                return Ok(CreatedMatch {
                    id,
                    first_invite,
                    second_invite,
                });
            }
            n += 1;
        }
    }

    fn mint_invites(&self, match_id: i64) -> rusqlite::Result<(String, String)> {
        let first = random_invite_token()?;
        let second = random_invite_token()?;
        for (seat, token) in [(1_i64, &first), (2_i64, &second)] {
            self.db.execute(
                "INSERT INTO agent_seats(match_id, seat, token_hash) VALUES(?1, ?2, ?3)
                 ON CONFLICT(match_id, seat) DO UPDATE SET token_hash = excluded.token_hash, registered = 0, ready = 0",
                params![match_id, seat, hash_token(token)],
            )?;
        }
        Ok((first, second))
    }

    pub(crate) fn rotate_invites(&self, match_id: i64) -> Result<(String, String), String> {
        let record = self
            .get(match_id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        if record.status == "Finished" || record.status == "In progress" {
            return Err(
                "Invitations can only be refreshed for waiting or paused matches".to_owned(),
            );
        }
        self.mint_invites(match_id).map_err(|e| e.to_string())
    }

    pub(crate) fn recover_after_restart(&self) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE matches SET status = 'Paused', updated_at = ?1 WHERE status = 'In progress'",
            [now()],
        )?;
        self.db.execute("UPDATE agent_seats SET registered = 0, ready = 0 WHERE match_id IN (SELECT id FROM matches WHERE status = 'Paused')", [])?;
        Ok(())
    }

    fn rename(&self, id: i64, name: &str) -> rusqlite::Result<()> {
        let clean = name.trim();
        if clean.is_empty() || clean.chars().count() > 48 {
            return Err(rusqlite::Error::InvalidParameterName(
                "Match name must be 1 to 48 characters".into(),
            ));
        }
        let other_names = {
            let mut statement = self.db.prepare("SELECT name FROM matches WHERE id != ?1")?;
            let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if other_names
            .iter()
            .any(|other| normalize_name(other) == normalize_name(clean))
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "That match name is already in use".into(),
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
        self.db.execute("UPDATE matches SET status = 'Paused', updated_at = ?1 WHERE id = ?2 AND status IN ('Waiting', 'In progress')", params![now(), id])?;
        self.db.execute(
            "UPDATE agent_seats SET registered = 0, ready = 0 WHERE match_id = ?1",
            [id],
        )?;
        Ok(())
    }

    fn set_status(&self, id: i64, status: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "UPDATE matches SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now(), id],
        )?;
        Ok(())
    }

    #[allow(dead_code)] // The authenticated agent API will call this in Phase 2.
    fn play_uci(&self, id: i64, uci: &str) -> Result<PlayedMove, String> {
        let record = self
            .get(id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        if record.status != "In progress" {
            return Err("Only an active match accepts moves".to_owned());
        }
        let mut game = Game::from_uci_history(&record.moves).map_err(|e| e.to_string())?;
        let played = game.play_uci(uci).map_err(|e| e.to_string())?;
        let status = match game.outcome() {
            Outcome::Checkmate { .. } | Outcome::Stalemate | Outcome::DrawAutomatic => "Finished",
            Outcome::Ongoing | Outcome::DrawClaimAvailable => "In progress",
        };
        self.db
            .execute(
                "UPDATE matches SET fen = ?1, moves = ?2, status = ?3, updated_at = ?4 WHERE id = ?5",
                params![game.fen(), game.uci_history(), status, now(), id],
            )
            .map_err(|e| e.to_string())?;
        Ok(played)
    }
}

struct ChessrenaApp {
    store: Store,
    invites: HashMap<i64, (String, String)>,
    selected: Option<i64>,
    rename_target: Option<(i64, String)>,
    delete_target: Option<i64>,
    error: Option<String>,
}

impl ChessrenaApp {
    fn new() -> Result<Self, String> {
        let path = data_dir()?.join("chessrena.db");
        let store = Store::open(path.clone())
            .map_err(|e| format!("Could not open local match database: {e}"))?;
        store.recover_after_restart().map_err(|e| e.to_string())?;
        api::spawn(path);
        Ok(Self {
            store,
            invites: HashMap::new(),
            selected: None,
            rename_target: None,
            delete_target: None,
            error: None,
        })
    }

    fn show_board(ui: &mut egui::Ui, fen: &str, moves: &str, size: f32, coordinates: bool) {
        let Ok(position) = Game::from_fen(fen) else {
            ui.colored_label(egui::Color32::LIGHT_RED, "Position data could not be read.");
            return;
        };
        let board = position.board();
        let last_move = Game::from_uci_history(moves)
            .ok()
            .and_then(|game| game.history().last().cloned())
            .and_then(|played| chess::ChessMove::from_str(&played.uci).ok());
        let square_size = size / 8.0;
        egui::Grid::new(ui.next_auto_id())
            .spacing(egui::vec2(0.0, 0.0))
            .show(ui, |ui| {
                for r in 0..8 {
                    if coordinates {
                        ui.allocate_ui_with_layout(
                            egui::vec2(16.0, square_size),
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                ui.label(
                                    egui::RichText::new((8 - r).to_string())
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(156, 169, 151)),
                                );
                            },
                        );
                    }
                    for f in 0..8 {
                        let square =
                            Square::make_square(Rank::from_index(7 - r), File::from_index(f));
                        let light = (r + f) % 2 == 0;
                        let highlighted = last_move
                            .is_some_and(|m| m.get_source() == square || m.get_dest() == square);
                        let color = match (light, highlighted) {
                            (true, false) => egui::Color32::from_rgb(238, 238, 210),
                            (false, false) => egui::Color32::from_rgb(118, 150, 86),
                            (true, true) => egui::Color32::from_rgb(246, 246, 105),
                            (false, true) => egui::Color32::from_rgb(186, 202, 43),
                        };
                        let piece = board
                            .piece_on(square)
                            .map(|p| piece_glyph(p, board.color_on(square).unwrap_or(Color::White)))
                            .unwrap_or(' ');
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(square_size, square_size),
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
                                egui::FontId::proportional(square_size * 0.78),
                                ink,
                            );
                        }
                    }
                    ui.end_row();
                }
                if coordinates {
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    for f in 0..8 {
                        let file = char::from(b'a' + f as u8);
                        ui.allocate_ui_with_layout(
                            egui::vec2(square_size, 16.0),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(file.to_string())
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(156, 169, 151)),
                                );
                            },
                        );
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
    fn open_saved_match(&mut self, id: i64) {
        if !self.invites.contains_key(&id) {
            match self.store.get(id) {
                Ok(Some(record)) if record.status == "Waiting" || record.status == "Paused" => {
                    match self.store.rotate_invites(id) {
                        Ok(tokens) => {
                            self.invites.insert(id, tokens);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            return;
                        }
                    }
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            }
        }
        self.selected = Some(id);
    }

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
                    match self.store.create_with_invites() {
                        Ok(created) => {
                            self.invites
                                .insert(created.id, (created.first_invite, created.second_invite));
                            self.selected = Some(created.id);
                        }
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
                                        Self::show_board(ui, &m.fen, &m.moves, 90.0, false);
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
                                                    self.open_saved_match(m.id);
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
                                                self.open_saved_match(m.id);
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
        let game = Game::from_uci_history(&record.moves).unwrap_or_default();
        let seats = self.store.seats(id).unwrap_or_default();
        let both_ready = seats.len() == 2 && seats.iter().all(|seat| seat.registered && seat.ready);
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
                    ui.label(
                        egui::RichText::new(format!(
                            "{} to move. {} legal moves.",
                            if game.side_to_move() == Color::White {
                                "White"
                            } else {
                                "Black"
                            },
                            game.legal_moves_uci().len()
                        ))
                        .strong(),
                    );
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(surface)
                        .stroke(egui::Stroke::new(1.0_f32, border))
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            Self::show_board(ui, &record.fen, &record.moves, 520.0, true)
                        });
                });
            });
            cols[1].vertical(|ui| {
                egui::Frame::group(ui.style())
                    .fill(surface)
                    .stroke(egui::Stroke::new(1.0_f32, border))
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("AGENTS")
                                .small()
                                .strong()
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(8.0);
                        for seat_number in 1..=2 {
                            let seat = seats.iter().find(|item| item.seat == seat_number);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Agent {seat_number}")).strong(),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let status = match seat {
                                            Some(item) if item.ready => "Ready",
                                            Some(item) if item.registered => "Registered",
                                            _ => "Waiting",
                                        };
                                        ui.label(
                                            egui::RichText::new(status)
                                                .small()
                                                .color(egui::Color32::GRAY),
                                        );
                                    },
                                );
                            });
                            if let Some(name) = seat.and_then(|item| item.name.as_deref()) {
                                ui.label(
                                    egui::RichText::new(name).small().color(egui::Color32::GRAY),
                                );
                            }
                            let token = self.invites.get(&id).map(|pair| {
                                if seat_number == 1 {
                                    pair.0.clone()
                                } else {
                                    pair.1.clone()
                                }
                            });
                            if let Some(token) = token {
                                if ui
                                    .button(format!("Copy prompt for Agent {seat_number}"))
                                    .clicked()
                                {
                                    let prompt = agent_prompt(id, seat_number, &token);
                                    ui.ctx().copy_text(prompt);
                                }
                            }
                            if seat_number == 1 {
                                ui.separator();
                            }
                        }
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(
                                "API is local only until the temporary tunnel phase is connected.",
                            )
                            .small()
                            .color(egui::Color32::GRAY),
                        );
                    });
                ui.add_space(12.0);
                egui::Frame::group(ui.style())
                    .fill(surface)
                    .stroke(egui::Stroke::new(1.0_f32, border))
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("MOVE HISTORY")
                                .small()
                                .strong()
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(8.0);
                        let history = game.san_history();
                        ui.label(if history.is_empty() {
                            "No moves yet".to_owned()
                        } else {
                            history
                        });
                    });
                ui.add_space(12.0);
                if record.status == "Paused" || record.status == "Waiting" {
                    let label = if record.status == "Paused" {
                        "Resume match"
                    } else {
                        "Start match"
                    };
                    if ui
                        .add_enabled(both_ready, egui::Button::new(label).fill(accent))
                        .clicked()
                    {
                        if let Err(error) = self.store.start_match(id) {
                            self.error = Some(error);
                        }
                    }
                    if !both_ready {
                        ui.label(
                            egui::RichText::new("Both agents must register and mark ready.")
                                .small()
                                .color(egui::Color32::GRAY),
                        );
                    }
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

fn agent_prompt(match_id: i64, seat: u8, token: &str) -> String {
    format!("You are one of two agents playing a Chessrena match. Use the official Python helper at https://github.com/goldggs/Chessrena/blob/main/agent/chessrena_agent.py. Save it in your persistent workspace and inspect the source before running it.\n\nRegister your distinct display name with:\npython3 chessrena_agent.py register --url http://127.0.0.1:18764 --match {match_id} --invite {token} --name YOUR_NAME\n\nThen run:\npython3 chessrena_agent.py ready --match {match_id}\npython3 chessrena_agent.py wait --match {match_id}\npython3 chessrena_agent.py move --match {match_id} UCI_MOVE\n\nOnly submit a move listed in the latest legalMoves data, and only when nextAction is move. The local API is not yet exposed through the temporary tunnel. Replace the local URL with the live tunnel address before using this prompt from a remote Arena agent. Your seat is Agent {seat}.")
}

fn random_invite_token() -> rusqlite::Result<String> {
    let mut bytes = [0_u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

fn normalize_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn last_move(moves: &str) -> String {
    Game::from_uci_history(moves)
        .ok()
        .and_then(|game| game.history().last().map(|played| played.san.clone()))
        .unwrap_or_else(|| "No moves".to_owned())
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
    fn match_name_uniqueness_handles_unicode_case() {
        let store = memory_store();
        let first = store.create().unwrap();
        let second = store.create().unwrap();
        store.rename(first, "Ägents").unwrap();
        assert!(store.rename(second, "ägents").is_err());
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
    fn store_persists_legal_moves_and_rejects_illegal_moves() {
        let store = memory_store();
        let id = store.create().unwrap();
        store.set_status(id, "In progress").unwrap();
        let played = store.play_uci(id, "e2e4").unwrap();
        assert_eq!(played.san, "e4");
        assert_eq!(store.get(id).unwrap().unwrap().moves, "e2e4");
        assert!(store.play_uci(id, "e2e3").is_err());
        assert_eq!(store.get(id).unwrap().unwrap().moves, "e2e4");
    }

    #[test]
    fn agents_register_uniquely_and_must_both_be_ready_to_start() {
        let store = memory_store();
        let created = store.create_with_invites().unwrap();
        store
            .register_agent(created.id, &created.first_invite, "North")
            .unwrap();
        assert!(store
            .register_agent(created.id, &created.second_invite, " north ")
            .is_err());
        store
            .register_agent(created.id, &created.second_invite, "South")
            .unwrap();
        store
            .mark_agent_ready(created.id, &created.first_invite)
            .unwrap();
        assert!(store.start_match(created.id).is_err());
        store
            .mark_agent_ready(created.id, &created.second_invite)
            .unwrap();
        store.start_match(created.id).unwrap();
        let seats = store.seats(created.id).unwrap();
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().all(|seat| seat.color.is_some()));
        assert_ne!(seats[0].color, seats[1].color);
        assert_eq!(
            store.get(created.id).unwrap().unwrap().status,
            "In progress"
        );
        let assigned_colors: Vec<_> = seats.iter().map(|seat| seat.color.clone()).collect();
        store.close_match(created.id).unwrap();
        assert!(store
            .seats(created.id)
            .unwrap()
            .iter()
            .all(|seat| !seat.registered && !seat.ready));
        store
            .register_agent(created.id, &created.first_invite, "North")
            .unwrap();
        store
            .register_agent(created.id, &created.second_invite, "South")
            .unwrap();
        store
            .mark_agent_ready(created.id, &created.first_invite)
            .unwrap();
        store
            .mark_agent_ready(created.id, &created.second_invite)
            .unwrap();
        store.start_match(created.id).unwrap();
        let resumed_colors: Vec<_> = store
            .seats(created.id)
            .unwrap()
            .iter()
            .map(|seat| seat.color.clone())
            .collect();
        assert_eq!(assigned_colors, resumed_colors);
    }

    #[test]
    fn invitation_tokens_are_hashed_at_rest() {
        let store = memory_store();
        let created = store.create_with_invites().unwrap();
        let stored_hash: String = store
            .db
            .query_row(
                "SELECT token_hash FROM agent_seats WHERE match_id = ?1 AND seat = 1",
                [created.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_hash.len(), 64);
        assert_ne!(stored_hash, created.first_invite);
    }

    #[test]
    fn invitation_rotation_revokes_the_previous_token() {
        let store = memory_store();
        let created = store.create_with_invites().unwrap();
        let old_token = created.first_invite;
        let (new_token, _) = store.rotate_invites(created.id).unwrap();
        assert!(store.seat_for_token(created.id, &old_token).is_err());
        assert_eq!(
            store.seat_for_token(created.id, &new_token).unwrap().seat,
            1
        );
    }

    #[test]
    fn initial_chess_position_has_twenty_legal_moves() {
        assert_eq!(MoveGen::new_legal(&Board::default()).count(), 20);
    }
}
