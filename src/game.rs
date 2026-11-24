use chess::{Board, BoardStatus, ChessMove, Color, File, MoveGen, Piece, Rank, Square};
use rand::{rngs::OsRng, RngCore};
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayedMove {
    pub uci: String,
    pub san: String,
    pub fen_after: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Ongoing,
    Checkmate { winner: Color },
    Stalemate,
    DrawClaimAvailable,
    DrawAutomatic,
}

/// Select which registration slot receives White using the operating system CSPRNG.
pub fn first_seat_gets_white() -> Result<bool, rand::Error> {
    let mut rng = OsRng;
    let mut byte = [0_u8; 1];
    rng.try_fill_bytes(&mut byte)?;
    Ok(byte[0] & 1 == 0)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MoveError {
    InvalidNotation,
    IllegalMove,
    GameOver,
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNotation => write!(f, "Move must use UCI notation such as e2e4 or e7e8q"),
            Self::IllegalMove => write!(f, "Move is not legal in the current position"),
            Self::GameOver => write!(f, "The game has ended"),
        }
    }
}

impl std::error::Error for MoveError {}

#[derive(Clone, Debug)]
pub struct Game {
    board: Board,
    halfmove_clock: u32,
    fullmove_number: u32,
    history: Vec<PlayedMove>,
    position_keys: Vec<String>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub fn new() -> Self {
        let board = Board::default();
        let mut game = Self {
            board,
            halfmove_clock: 0,
            fullmove_number: 1,
            history: Vec::new(),
            position_keys: Vec::new(),
        };
        game.position_keys.push(game.position_key());
        game
    }

    pub fn from_fen(fen: &str) -> Result<Self, MoveError> {
        let board = Board::from_str(fen).map_err(|_| MoveError::InvalidNotation)?;
        let fields: Vec<&str> = fen.split_whitespace().collect();
        let halfmove_clock = fields.get(4).and_then(|x| x.parse().ok()).unwrap_or(0);
        let fullmove_number = fields
            .get(5)
            .and_then(|x| x.parse().ok())
            .unwrap_or(1)
            .max(1);
        let mut game = Self {
            board,
            halfmove_clock,
            fullmove_number,
            history: Vec::new(),
            position_keys: Vec::new(),
        };
        game.position_keys.push(game.position_key());
        Ok(game)
    }

    pub fn from_uci_history(uci_history: &str) -> Result<Self, MoveError> {
        let mut game = Self::new();
        for uci in uci_history.split_whitespace() {
            game.play_uci(uci)?;
        }
        Ok(game)
    }

    pub fn fen(&self) -> String {
        let base = self.board.to_string();
        let fields: Vec<&str> = base.split_whitespace().collect();
        format!(
            "{} {} {}",
            fields[..4].join(" "),
            self.halfmove_clock,
            self.fullmove_number
        )
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn history(&self) -> &[PlayedMove] {
        &self.history
    }

    #[allow(dead_code)] // Stored move history is consumed by the agent API.
    pub fn uci_history(&self) -> String {
        self.history
            .iter()
            .map(|m| m.uci.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn san_history(&self) -> String {
        let mut out = Vec::new();
        for (index, ply) in self.history.iter().enumerate() {
            if index % 2 == 0 {
                out.push(format!("{}. {}", index / 2 + 1, ply.san));
            } else if let Some(last) = out.last_mut() {
                last.push_str(&format!(" {}", ply.san));
            }
        }
        out.join("  ")
    }

    pub fn legal_moves_uci(&self) -> Vec<String> {
        MoveGen::new_legal(&self.board)
            .map(|m| m.to_string())
            .collect()
    }

    pub fn side_to_move(&self) -> Color {
        self.board.side_to_move()
    }

    pub fn outcome(&self) -> Outcome {
        match self.board.status() {
            BoardStatus::Checkmate => {
                return Outcome::Checkmate {
                    winner: !self.board.side_to_move(),
                }
            }
            BoardStatus::Stalemate => return Outcome::Stalemate,
            BoardStatus::Ongoing => {}
        }
        let current_key = self.position_key();
        let repetitions = self
            .position_keys
            .iter()
            .filter(|key| **key == current_key)
            .count();
        if repetitions >= 5 || self.halfmove_clock >= 150 {
            Outcome::DrawAutomatic
        } else if repetitions >= 3 || self.halfmove_clock >= 100 {
            Outcome::DrawClaimAvailable
        } else {
            Outcome::Ongoing
        }
    }

    pub fn play_uci(&mut self, uci: &str) -> Result<PlayedMove, MoveError> {
        if matches!(
            self.outcome(),
            Outcome::Checkmate { .. } | Outcome::Stalemate | Outcome::DrawAutomatic
        ) {
            return Err(MoveError::GameOver);
        }
        let chess_move = ChessMove::from_str(uci).map_err(|_| MoveError::InvalidNotation)?;
        if !MoveGen::new_legal(&self.board).any(|candidate| candidate == chess_move) {
            return Err(MoveError::IllegalMove);
        }

        let moving_piece = self
            .board
            .piece_on(chess_move.get_source())
            .ok_or(MoveError::IllegalMove)?;
        let capture = self.board.piece_on(chess_move.get_dest()).is_some()
            || (moving_piece == Piece::Pawn
                && chess_move.get_source().get_file() != chess_move.get_dest().get_file());
        let san_base = self.san_base(chess_move, moving_piece, capture);
        if moving_piece == Piece::Pawn || capture {
            self.halfmove_clock = 0;
        } else {
            self.halfmove_clock = self.halfmove_clock.saturating_add(1);
        }
        if self.board.side_to_move() == Color::Black {
            self.fullmove_number = self.fullmove_number.saturating_add(1);
        }
        self.board = self.board.make_move_new(chess_move);
        let suffix = match self.board.status() {
            BoardStatus::Checkmate => "#",
            BoardStatus::Ongoing if self.board.checkers().popcnt() > 0 => "+",
            _ => "",
        };
        let played = PlayedMove {
            uci: chess_move.to_string(),
            san: format!("{san_base}{suffix}"),
            fen_after: self.fen(),
        };
        self.history.push(played.clone());
        self.position_keys.push(self.position_key());
        Ok(played)
    }

    fn san_base(&self, chess_move: ChessMove, piece: Piece, capture: bool) -> String {
        let source = chess_move.get_source();
        let dest = chess_move.get_dest();
        if piece == Piece::King
            && source
                .get_file()
                .to_index()
                .abs_diff(dest.get_file().to_index())
                == 2
        {
            return if dest.get_file().to_index() > source.get_file().to_index() {
                "O-O"
            } else {
                "O-O-O"
            }
            .to_owned();
        }

        let mut san = String::new();
        if piece == Piece::Pawn {
            if capture {
                san.push(file_char(source.get_file()));
            }
        } else {
            san.push(piece_letter(piece));
            let alternatives: Vec<ChessMove> = MoveGen::new_legal(&self.board)
                .filter(|m| {
                    *m != chess_move
                        && m.get_dest() == dest
                        && self.board.piece_on(m.get_source()) == Some(piece)
                })
                .collect();
            if !alternatives.is_empty() {
                let same_file = alternatives
                    .iter()
                    .any(|m| m.get_source().get_file() == source.get_file());
                let same_rank = alternatives
                    .iter()
                    .any(|m| m.get_source().get_rank() == source.get_rank());
                if !same_file {
                    san.push(file_char(source.get_file()));
                } else if !same_rank {
                    san.push(rank_char(source.get_rank()));
                } else {
                    san.push(file_char(source.get_file()));
                    san.push(rank_char(source.get_rank()));
                }
            }
        }
        if capture {
            san.push('x');
        }
        san.push_str(&square_name(dest));
        if let Some(promotion) = chess_move.get_promotion() {
            san.push('=');
            san.push(piece_letter(promotion));
        }
        san
    }

    fn position_key(&self) -> String {
        self.board
            .to_string()
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn file_char(file: File) -> char {
    (b'a' + file.to_index() as u8) as char
}

fn rank_char(rank: Rank) -> char {
    (b'1' + rank.to_index() as u8) as char
}

fn square_name(square: Square) -> String {
    format!(
        "{}{}",
        file_char(square.get_file()),
        rank_char(square.get_rank())
    )
}

fn piece_letter(piece: Piece) -> char {
    match piece {
        Piece::King => 'K',
        Piece::Queen => 'Q',
        Piece::Rook => 'R',
        Piece::Bishop => 'B',
        Piece::Knight => 'N',
        Piece::Pawn => 'P',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_color_assignment_uses_os_randomness() {
        assert!(first_seat_gets_white().is_ok());
    }

    #[test]
    fn san_and_fen_track_a_normal_turn() {
        let mut game = Game::new();
        assert_eq!(game.play_uci("e2e4").unwrap().san, "e4");
        assert_eq!(game.play_uci("e7e5").unwrap().san, "e5");
        let fields: Vec<_> = game.fen().split_whitespace().map(str::to_owned).collect();
        assert_eq!(fields[1], "w");
        assert_eq!(fields[4], "0");
        assert_eq!(fields[5], "2");
        assert_eq!(game.san_history(), "1. e4 e5");
    }

    #[test]
    fn rejects_wrong_turn_and_illegal_moves() {
        let mut game = Game::new();
        assert_eq!(game.play_uci("e7e5"), Err(MoveError::IllegalMove));
        assert_eq!(game.play_uci("not-a-move"), Err(MoveError::InvalidNotation));
        assert_eq!(game.history().len(), 0);
    }

    #[test]
    fn recognizes_castling_san() {
        let mut game = Game::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(game.play_uci("e1g1").unwrap().san, "O-O");
    }

    #[test]
    fn disambiguates_piece_moves_in_san() {
        let mut game = Game::from_fen("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1").unwrap();
        assert_eq!(game.play_uci("b1d2").unwrap().san, "Nbd2");
    }

    #[test]
    fn records_promotion_and_en_passant() {
        let mut promotion = Game::from_fen("7k/P7/8/8/8/8/8/7K w - - 0 1").unwrap();
        assert_eq!(promotion.play_uci("a7a8q").unwrap().san, "a8=Q+");
        let mut en_passant = Game::from_fen("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2").unwrap();
        assert_eq!(en_passant.play_uci("e5d6").unwrap().san, "exd6");
    }

    #[test]
    fn recognizes_checkmate() {
        let mut game = Game::new();
        for uci in ["f2f3", "e7e5", "g2g4"] {
            game.play_uci(uci).unwrap();
        }
        assert_eq!(game.play_uci("d8h4").unwrap().san, "Qh4#");
        assert_eq!(
            game.outcome(),
            Outcome::Checkmate {
                winner: Color::Black
            }
        );
        assert_eq!(game.play_uci("e2e4"), Err(MoveError::GameOver));
    }

    #[test]
    fn recognizes_threefold_claim_availability() {
        let mut game = Game::new();
        for uci in [
            "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
        ] {
            game.play_uci(uci).unwrap();
        }
        assert_eq!(game.outcome(), Outcome::DrawClaimAvailable);
    }

    #[test]
    fn recognizes_seventy_five_move_automatic_draw() {
        let game = Game::from_fen("8/8/8/8/8/2k5/8/2K5 w - - 150 76").unwrap();
        assert_eq!(game.outcome(), Outcome::DrawAutomatic);
    }
}
