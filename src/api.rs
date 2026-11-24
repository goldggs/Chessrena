use crate::{piece_glyph, AgentSeat, ChatMessage, Store};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chess::{Color, File, Rank, Square};
use chessrena::game::{Game, Outcome};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

const ADDRESS: &str = "127.0.0.1:18764";

#[derive(Clone)]
struct ApiState {
    store: Arc<Mutex<Store>>,
}

#[derive(serde::Deserialize)]
struct RegisterRequest {
    name: String,
}

#[derive(serde::Deserialize)]
struct MoveRequest {
    uci: String,
    position_version: usize,
}

#[derive(serde::Deserialize)]
struct ChatRequest {
    text: String,
}

#[derive(serde::Deserialize)]
struct AcknowledgeRequest {
    message_id: i64,
    reply: Option<String>,
}

#[derive(serde::Serialize)]
struct ErrorBody {
    error: String,
}

type ApiResponse = Result<Json<Value>, (StatusCode, Json<ErrorBody>)>;

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ErrorBody>) {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, (StatusCode, Json<ErrorBody>)> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Bearer invitation token required"))
}

fn with_store<T>(
    state: &ApiState,
    f: impl FnOnce(&Store) -> Result<T, String>,
) -> Result<T, (StatusCode, Json<ErrorBody>)> {
    let guard = state
        .store
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Match store unavailable"))?;
    f(&guard).map_err(|message| {
        let code = if message.contains("not found") {
            StatusCode::NOT_FOUND
        } else if message.contains("Invalid") || message.contains("token") {
            StatusCode::UNAUTHORIZED
        } else if [
            "cannot",
            "must",
            "already",
            "legal",
            "turn",
            "waiting",
            "active",
            "ready",
            "position",
            "color",
            "register",
            "chat",
            "acknowledge",
            "closed",
            "resign",
            "started",
            "message",
            "ended",
        ]
        .iter()
        .any(|marker| message.to_lowercase().contains(marker))
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        error(code, message)
    })
}

async fn health() -> Json<Value> {
    Json(json!({"service": "Chessrena", "apiVersion": 1}))
}

async fn register(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<RegisterRequest>,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let seat = with_store(&state, |store| {
        store.register_agent(match_id, token, &request.name)
    })?;
    Ok(Json(json!({
        "matchId": match_id,
        "seat": seat.seat,
        "name": seat.name,
        "registered": seat.registered,
        "ready": seat.ready,
        "message": "Registration saved. Fetch state, then mark ready when prepared."
    })))
}

async fn ready(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let seat = with_store(&state, |store| store.mark_agent_ready(match_id, token))?;
    let seats = with_store(&state, |store| {
        store.seats(match_id).map_err(|e| e.to_string())
    })?;
    Ok(Json(json!({
        "matchId": match_id,
        "seat": seat.seat,
        "ready": seat.ready,
        "bothReady": seats.len() == 2 && seats.iter().all(|item| item.registered && item.ready),
        "message": "Ready status saved. The app owner must start or resume the match."
    })))
}

async fn get_agent_state(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let result = with_store(&state, |store| {
        let seat = store.seat_for_token(match_id, token)?;
        current_state(store, match_id, &seat)
    })?;
    Ok(Json(result))
}

async fn submit_move(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<MoveRequest>,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let result = with_store(&state, |store| {
        let seat = store.seat_for_token(match_id, token)?;
        let record = store
            .get(match_id)
            .map_err(|e| e.to_string())?
            .ok_or("Match not found")?;
        let game = Game::from_uci_history(&record.moves).map_err(|e| e.to_string())?;
        if store.has_pending_chat_for_seat(match_id, seat.seat)? {
            return Err("Acknowledge the pending chat message before moving".to_owned());
        }
        if request.position_version != game.history().len() {
            return Err(
                "Position version is stale. Fetch the current state before moving".to_owned(),
            );
        }
        let assigned_color = seat
            .color
            .as_deref()
            .ok_or("Agent has no assigned color yet")?;
        let expected_color = color_name(game.side_to_move());
        if record.status != "In progress" || assigned_color != expected_color {
            return Err("It is not this agent's turn in an active match".to_owned());
        }
        let played = store.play_uci(match_id, &request.uci)?;
        Ok(json!({
            "accepted": true,
            "move": {"uci": played.uci, "san": played.san},
            "state": current_state(store, match_id, &seat)?
        }))
    })?;
    Ok(Json(result))
}

async fn send_chat(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let result = with_store(&state, |store| {
        store.send_chat(match_id, token, &request.text)?;
        let seat = store.seat_for_token(match_id, token)?;
        current_state(store, match_id, &seat)
    })?;
    Ok(Json(json!({"accepted": true, "state": result})))
}

async fn acknowledge_chat(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
    Json(request): Json<AcknowledgeRequest>,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let result = with_store(&state, |store| {
        store.acknowledge_chat(
            match_id,
            token,
            request.message_id,
            request.reply.as_deref(),
        )?;
        let seat = store.seat_for_token(match_id, token)?;
        current_state(store, match_id, &seat)
    })?;
    Ok(Json(json!({"accepted": true, "state": result})))
}

async fn resign(
    State(state): State<ApiState>,
    Path(match_id): Path<i64>,
    headers: HeaderMap,
) -> ApiResponse {
    let token = bearer_token(&headers)?;
    let result = with_store(&state, |store| {
        store.resign(match_id, token)?;
        let seat = store.seat_for_token(match_id, token)?;
        current_state(store, match_id, &seat)
    })?;
    Ok(Json(json!({"accepted": true, "state": result})))
}

fn current_state(store: &Store, match_id: i64, seat: &AgentSeat) -> Result<Value, String> {
    let record = store
        .get(match_id)
        .map_err(|e| e.to_string())?
        .ok_or("Match not found")?;
    let game = Game::from_uci_history(&record.moves).map_err(|e| e.to_string())?;
    let chat_enabled = store.chat_enabled(match_id)?;
    let messages = store.chat_messages(match_id).map_err(|e| e.to_string())?;
    Ok(state_json(
        match_id,
        &record,
        seat,
        &game,
        chat_enabled,
        &messages,
    ))
}

fn color_name(color: Color) -> &'static str {
    if color == Color::White {
        "white"
    } else {
        "black"
    }
}

fn state_json(
    match_id: i64,
    record: &crate::MatchRecord,
    seat: &AgentSeat,
    game: &Game,
    chat_enabled: bool,
    chat_messages: &[ChatMessage],
) -> Value {
    let side_to_move = color_name(game.side_to_move());
    let pending_for_you = chat_messages
        .iter()
        .find(|message| message.acknowledged_at.is_none() && message.sender_seat != seat.seat);
    let any_pending = chat_messages
        .iter()
        .any(|message| message.acknowledged_at.is_none());
    let next_action = if !record.is_open {
        "closed"
    } else if !seat.registered {
        "register"
    } else if chat_enabled && pending_for_you.is_some() {
        "acknowledge"
    } else {
        match record.status.as_str() {
            "Finished" => "finished",
            "Paused" => "paused",
            "In progress" if seat.color.as_deref() == Some(side_to_move) => "move",
            "In progress" => "wait",
            "Waiting" => "wait",
            _ => "wait",
        }
    };
    let legal_moves = if next_action == "move" && chat_enabled && pending_for_you.is_none() {
        game.legal_moves()
            .into_iter()
            .map(|m| json!({"uci": m.uci, "san": m.san}))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let outcome = match game.outcome() {
        Outcome::Ongoing => "ongoing",
        Outcome::Checkmate { .. } => "checkmate",
        Outcome::Stalemate => "stalemate",
        Outcome::DrawClaimAvailable => "draw_claim_available",
        Outcome::DrawAutomatic => "draw",
    };
    let moves: Vec<Value> = game
        .history()
        .iter()
        .enumerate()
        .map(|(index, item)| {
            json!({
                "ply": index + 1, "uci": item.uci, "san": item.san, "fenAfter": item.fen_after
            })
        })
        .collect();
    let chat: Vec<Value> = chat_messages
        .iter()
        .map(|message| {
            json!({
                "id": message.id,
                "senderSeat": message.sender_seat,
                "senderName": message.sender_name,
                "text": message.text,
                "createdAt": message.created_at,
                "acknowledgedAt": message.acknowledged_at,
            })
        })
        .collect();
    let pending_message = pending_for_you.map(|message| {
        json!({
            "id": message.id,
            "senderSeat": message.sender_seat,
            "senderName": message.sender_name,
            "text": message.text,
            "createdAt": message.created_at,
        })
    });
    let can_resign = chat_enabled
        && ["In progress", "Paused"].contains(&record.status.as_str())
        && seat.color.is_some();
    json!({
        "schemaVersion": 1,
        "match": {
            "id": match_id, "name": record.name, "status": record.status, "open": record.is_open,
            "result": record.result, "winnerColor": record.winner_color,
            "positionVersion": game.history().len(), "ply": game.history().len(), "outcome": outcome
        },
        "agent": {
            "seat": seat.seat, "name": seat.name, "color": seat.color,
            "registered": seat.registered, "ready": seat.ready
        },
        "nextAction": next_action,
        "pollAfterSeconds": if next_action == "wait" { Some(3) } else { None },
        "chat": {
            "enabled": chat_enabled,
            "canSend": chat_enabled && !any_pending,
            "pendingForYou": pending_message,
            "messages": chat
        },
        "actions": {"canResign": can_resign},
        "position": {
            "fen": game.fen(), "sideToMove": side_to_move, "inCheck": game.in_check(),
            "halfmoveClock": game.halfmove_clock(), "repetitionCount": game.repetition_count(),
            "drawClaimAvailable": outcome == "draw_claim_available",
            "lastMove": game.history().last().map(|m| json!({"uci": m.uci, "san": m.san})),
            "moveHistory": moves, "legalMoves": legal_moves
        },
        "boardSvg": board_svg(game)
    })
}

fn board_svg(game: &Game) -> String {
    let mut svg = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 520 520\" role=\"img\" aria-label=\"Chess board position\"><rect width=\"520\" height=\"520\" fill=\"#111412\"/>");
    for row in 0..8 {
        for file in 0..8 {
            let square = Square::make_square(Rank::from_index(7 - row), File::from_index(file));
            let light = (row + file) % 2 == 0;
            let highlighted = game
                .history()
                .last()
                .and_then(|m| chess::ChessMove::from_str(&m.uci).ok())
                .is_some_and(|m| m.get_source() == square || m.get_dest() == square);
            let fill = match (light, highlighted) {
                (true, false) => "#eeeed2",
                (false, false) => "#769656",
                (true, true) => "#f6f669",
                (false, true) => "#baca2b",
            };
            let x = 24 + file * 60;
            let y = 24 + row * 60;
            svg.push_str(&format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"60\" height=\"60\" fill=\"{fill}\"/>"
            ));
            if let Some(piece) = game.board().piece_on(square) {
                let glyph = piece_glyph(piece);
                let is_white = game.board().color_on(square) == Some(Color::White);
                let fill = if is_white { "#faf9f2" } else { "#1f2721" };
                let stroke = if is_white { "#263026" } else { "#1f2721" };
                svg.push_str(&format!("<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" dominant-baseline=\"central\" font-family=\"Segoe UI Symbol, DejaVu Sans, serif\" font-size=\"48\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.4\" paint-order=\"stroke fill\" stroke-linejoin=\"round\">{glyph}</text>", x + 30, y + 31));
            }
        }
        svg.push_str(&format!("<text x=\"10\" y=\"{}\" font-family=\"system-ui, sans-serif\" font-size=\"11\" fill=\"#a1aaa3\" text-anchor=\"middle\" dominant-baseline=\"central\">{}</text>", 54 + row * 60, 8 - row));
    }
    for file in 0..8 {
        svg.push_str(&format!("<text x=\"{}\" y=\"510\" font-family=\"system-ui, sans-serif\" font-size=\"11\" fill=\"#a1aaa3\" text-anchor=\"middle\">{}</text>", 54 + file * 60, char::from(b'a' + file as u8)));
    }
    svg.push_str("</svg>");
    svg
}

fn make_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/matches/:match_id/register", post(register))
        .route("/api/matches/:match_id/ready", post(ready))
        .route("/api/matches/:match_id/state", get(get_agent_state))
        .route("/api/matches/:match_id/move", post(submit_move))
        .route("/api/matches/:match_id/chat", post(send_chat))
        .route("/api/matches/:match_id/chat/ack", post(acknowledge_chat))
        .route("/api/matches/:match_id/resign", post(resign))
        .with_state(state)
}

pub(crate) fn spawn(database_path: PathBuf) -> Result<(), String> {
    let store = Store::open(database_path)
        .map_err(|error| format!("Chessrena API could not open match database: {error}"))?;
    let listener = std::net::TcpListener::bind(ADDRESS)
        .map_err(|error| format!("Chessrena API could not bind {ADDRESS}: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the local API listener: {error}"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Chessrena API runtime error: {error}"))?;
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("chessrena-api".to_owned())
        .spawn(move || {
            runtime.block_on(async move {
                let app = make_router(ApiState {
                    store: Arc::new(Mutex::new(store)),
                });
                let listener = match TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        eprintln!("Chessrena API could not initialize listener: {error}");
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                if let Err(error) = axum::serve(listener, app).await {
                    eprintln!("Chessrena API stopped: {error}");
                }
            });
        })
        .map_err(|error| format!("Could not start the Chessrena API thread: {error}"))?;
    ready_rx
        .recv()
        .map_err(|error| format!("Chessrena API failed to initialize: {error}"))?
        .map_err(|error| format!("Chessrena API could not initialize listener: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    fn running_match() -> (Router, i64, String, String) {
        let store =
            Store::from_connection(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        let created = store.create_with_invites().unwrap();
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
        let seats = store.seats(created.id).unwrap();
        let first_white = seats
            .iter()
            .find(|seat| seat.seat == 1)
            .unwrap()
            .color
            .as_deref()
            == Some("white");
        let (white_token, black_token) = if first_white {
            (created.first_invite, created.second_invite)
        } else {
            (created.second_invite, created.first_invite)
        };
        (
            make_router(ApiState {
                store: Arc::new(Mutex::new(store)),
            }),
            created.id,
            white_token,
            black_token,
        )
    }

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if body.is_some() {
            request = request.header(axum::http::header::CONTENT_TYPE, "application/json");
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(body.unwrap_or("").to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn rejects_missing_and_invalid_credentials() {
        let (app, match_id, _, _) = running_match();
        let path = format!("/api/matches/{match_id}/state");
        assert_eq!(
            call(&app, "GET", &path, None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "GET", &path, Some("wrong-token"), None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn chat_acknowledgment_gates_only_the_recipient_move() {
        let (app, match_id, white_token, black_token) = running_match();
        let chat_path = format!("/api/matches/{match_id}/chat");
        let ack_path = format!("/api/matches/{match_id}/chat/ack");
        let move_path = format!("/api/matches/{match_id}/move");
        let state_path = format!("/api/matches/{match_id}/state");
        let (sent, sent_body) = call(
            &app,
            "POST",
            &chat_path,
            Some(&white_token),
            Some(r#"{"text":"heh, you don't know what's coming"}"#),
        )
        .await;
        assert_eq!(sent, StatusCode::OK);
        assert_eq!(sent_body["state"]["chat"]["canSend"], false);

        let (_, black_state) = call(&app, "GET", &state_path, Some(&black_token), None).await;
        assert_eq!(black_state["nextAction"], "acknowledge");
        assert_eq!(
            black_state["chat"]["pendingForYou"]["text"],
            "heh, you don't know what's coming"
        );

        let (white_move, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&white_token),
            Some(r#"{"uci":"e2e4","position_version":0}"#),
        )
        .await;
        assert_eq!(white_move, StatusCode::OK);
        let (blocked, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&black_token),
            Some(r#"{"uci":"e7e5","position_version":1}"#),
        )
        .await;
        assert_eq!(blocked, StatusCode::CONFLICT);

        let message_id = black_state["chat"]["pendingForYou"]["id"].as_i64().unwrap();
        let (acked, ack_body) = call(
            &app,
            "POST",
            &ack_path,
            Some(&black_token),
            Some(&format!(r#"{{"message_id":{message_id},"reply":"nice"}}"#)),
        )
        .await;
        assert_eq!(acked, StatusCode::OK);
        assert_eq!(ack_body["state"]["nextAction"], "move");
        assert_eq!(ack_body["state"]["chat"]["canSend"], false);
        let (black_move, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&black_token),
            Some(r#"{"uci":"e7e5","position_version":1}"#),
        )
        .await;
        assert_eq!(black_move, StatusCode::OK);
        let (_, white_state) = call(&app, "GET", &state_path, Some(&white_token), None).await;
        assert_eq!(white_state["nextAction"], "acknowledge");
        let reply_id = white_state["chat"]["pendingForYou"]["id"].as_i64().unwrap();
        let (reply_ack, _) = call(
            &app,
            "POST",
            &ack_path,
            Some(&white_token),
            Some(&format!(r#"{{"message_id":{reply_id},"reply":null}}"#)),
        )
        .await;
        assert_eq!(reply_ack, StatusCode::OK);
    }

    #[tokio::test]
    async fn resign_ends_with_winner_and_chat_remains_available() {
        let (app, match_id, white_token, black_token) = running_match();
        let resign_path = format!("/api/matches/{match_id}/resign");
        let chat_path = format!("/api/matches/{match_id}/chat");
        let (status, result) =
            call(&app, "POST", &resign_path, Some(&white_token), Some("{}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["state"]["match"]["result"], "resignation");
        assert_eq!(result["state"]["match"]["winnerColor"], "black");
        assert!(result["state"]["chat"]["enabled"].as_bool().unwrap());
        let (sent, _) = call(
            &app,
            "POST",
            &chat_path,
            Some(&black_token),
            Some(r#"{"text":"gg"}"#),
        )
        .await;
        assert_eq!(sent, StatusCode::OK);
        let (second_resign, _) =
            call(&app, "POST", &resign_path, Some(&black_token), Some("{}")).await;
        assert_eq!(second_resign, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn only_current_seat_can_submit_fresh_legal_moves() {
        let (app, match_id, white_token, black_token) = running_match();
        let state_path = format!("/api/matches/{match_id}/state");
        let move_path = format!("/api/matches/{match_id}/move");
        let (status, white_state) = call(&app, "GET", &state_path, Some(&white_token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(white_state["nextAction"], "move");
        assert!(white_state["position"]["legalMoves"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["uci"] == "e2e4"));
        assert!(white_state["boardSvg"]
            .as_str()
            .unwrap()
            .starts_with("<svg"));
        let (wrong_turn, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&black_token),
            Some(r#"{"uci":"e7e5","position_version":0}"#),
        )
        .await;
        assert_eq!(wrong_turn, StatusCode::CONFLICT);
        let (illegal, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&white_token),
            Some(r#"{"uci":"e2e5","position_version":0}"#),
        )
        .await;
        assert_eq!(illegal, StatusCode::CONFLICT);
        let (accepted, move_result) = call(
            &app,
            "POST",
            &move_path,
            Some(&white_token),
            Some(r#"{"uci":"e2e4","position_version":0}"#),
        )
        .await;
        assert_eq!(accepted, StatusCode::OK);
        assert_eq!(move_result["accepted"], true);
        assert_eq!(move_result["move"]["san"], "e4");
        let (black_status, black_state) =
            call(&app, "GET", &state_path, Some(&black_token), None).await;
        assert_eq!(black_status, StatusCode::OK);
        assert_eq!(black_state["nextAction"], "move");
        assert_eq!(black_state["match"]["positionVersion"], 1);
        assert_eq!(black_state["position"]["sideToMove"], "black");
        let (stale, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&black_token),
            Some(r#"{"uci":"e7e5","position_version":0}"#),
        )
        .await;
        assert_eq!(stale, StatusCode::CONFLICT);
        let (black_move, _) = call(
            &app,
            "POST",
            &move_path,
            Some(&black_token),
            Some(r#"{"uci":"e7e5","position_version":1}"#),
        )
        .await;
        assert_eq!(black_move, StatusCode::OK);
    }
}
