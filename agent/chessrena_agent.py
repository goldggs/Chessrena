#!/usr/bin/env python3
"""Small standard-library client for the Chessrena agent API."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

WORKSPACE = Path.cwd() / "chessrena_workspace"


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def validate_server(value: str) -> str:
    value = value.rstrip("/")
    parsed = urlsplit(value)
    local_http = parsed.scheme == "http" and parsed.hostname in {"127.0.0.1", "localhost", "::1"}
    if parsed.scheme != "https" and not local_http:
        raise ValueError("Use HTTPS. Plain HTTP is allowed only for loopback testing.")
    if not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("Server must be a base URL without credentials, query, or fragment.")
    return value


def config_path(match_id: int) -> Path:
    if match_id < 1:
        raise ValueError("Match ID must be a positive integer.")
    return WORKSPACE / f"match-{match_id}.json"


def save_config(path: Path, config: dict) -> None:
    WORKSPACE.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".config-", dir=WORKSPACE)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(config, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        try:
            os.chmod(temporary, 0o600)
        except OSError:
            pass
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def load_config(match_id: int) -> tuple[Path, dict]:
    path = config_path(match_id)
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ValueError(f"No saved config for match {match_id}. Run register first.") from exc
    config["server"] = validate_server(config["server"])
    return path, config


def request_json(server: str, path: str, token: str, method: str = "GET", payload: dict | None = None) -> dict:
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    headers = {"Authorization": f"Bearer {token}", "Accept": "application/json"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = Request(server + path, data=body, headers=headers, method=method)
    opener = build_opener(NoRedirect())
    try:
        with opener.open(request, timeout=15) as response:
            result = json.loads(response.read().decode("utf-8"))
    except HTTPError as exc:
        detail = ""
        try:
            detail = json.loads(exc.read().decode("utf-8")).get("error", "")
        except (ValueError, OSError):
            pass
        raise RuntimeError(detail or f"Chessrena returned HTTP {exc.code}.") from exc
    except (URLError, TimeoutError, OSError) as exc:
        raise RuntimeError(f"Could not reach Chessrena at {server}: {exc}") from exc
    if not isinstance(result, dict):
        raise RuntimeError("Chessrena returned an invalid response.")
    return result


def persist_state(path: Path, config: dict, state: dict) -> None:
    config["positionVersion"] = state.get("match", {}).get("positionVersion", 0)
    legal = state.get("position", {}).get("legalMoves", [])
    config["legalMoves"] = [item.get("uci") for item in legal if isinstance(item, dict) and item.get("uci")]
    save_config(path, config)
    svg = state.get("boardSvg")
    if isinstance(svg, str) and svg.startswith("<svg"):
        board_path = WORKSPACE / f"board-{config['matchId']}.svg"
        board_path.write_text(svg, encoding="utf-8")
        try:
            os.chmod(board_path, 0o600)
        except OSError:
            pass
        state["boardFile"] = str(board_path)


def command_register(args: argparse.Namespace) -> None:
    server = validate_server(args.url)
    if len(args.invite) < 32:
        raise ValueError("Invitation token is too short.")
    path = config_path(args.match)
    result = request_json(server, f"/api/matches/{args.match}/register", args.invite, "POST", {"name": args.name})
    config = {
        "server": server,
        "matchId": args.match,
        "token": args.invite,
        "name": args.name.strip(),
        "positionVersion": 0,
        "legalMoves": [],
    }
    save_config(path, config)
    result["configFile"] = str(path)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def command_ready(args: argparse.Namespace) -> None:
    _, config = load_config(args.match)
    result = request_json(config["server"], f"/api/matches/{args.match}/ready", config["token"], "POST", {})
    print(json.dumps(result, ensure_ascii=False, indent=2))


def command_state(args: argparse.Namespace) -> None:
    path, config = load_config(args.match)
    state = request_json(config["server"], f"/api/matches/{args.match}/state", config["token"])
    persist_state(path, config, state)
    print(json.dumps(state, ensure_ascii=False, indent=2))


def command_wait(args: argparse.Namespace) -> None:
    path, config = load_config(args.match)
    deadline = time.monotonic() + args.timeout
    while True:
        state = request_json(config["server"], f"/api/matches/{args.match}/state", config["token"])
        persist_state(path, config, state)
        action = state.get("nextAction")
        if action != "wait" or time.monotonic() >= deadline:
            print(json.dumps(state, ensure_ascii=False, indent=2))
            return
        time.sleep(min(args.interval, max(0.0, deadline - time.monotonic())))


def command_set_url(args: argparse.Namespace) -> None:
    path, config = load_config(args.match)
    config["server"] = validate_server(args.url)
    save_config(path, config)
    print(json.dumps({"matchId": args.match, "server": config["server"], "updated": True}, indent=2))


def command_move(args: argparse.Namespace) -> None:
    path, config = load_config(args.match)
    if args.uci not in config.get("legalMoves", []):
        raise ValueError("That move was not in the latest legalMoves list. Run wait or state to refresh first.")
    result = request_json(
        config["server"],
        f"/api/matches/{args.match}/move",
        config["token"],
        "POST",
        {"uci": args.uci, "position_version": config.get("positionVersion", 0)},
    )
    state = result.get("state")
    if isinstance(state, dict):
        persist_state(path, config, state)
    print(json.dumps(result, ensure_ascii=False, indent=2))


def main() -> int:
    parser = argparse.ArgumentParser(description="Talk to Chessrena using the agent's own workspace config.")
    commands = parser.add_subparsers(dest="command", required=True)

    register = commands.add_parser("register", help="Register this agent in a match.")
    register.add_argument("--url", required=True, help="Chessrena API base URL.")
    register.add_argument("--match", required=True, type=int, help="Match ID.")
    register.add_argument("--invite", required=True, help="Private invitation token for this agent slot.")
    register.add_argument("--name", required=True, help="Display name, unique within the match.")
    register.set_defaults(run=command_register)

    for name, handler in [("ready", command_ready), ("state", command_state)]:
        sub = commands.add_parser(name)
        sub.add_argument("--match", required=True, type=int)
        sub.set_defaults(run=handler)

    wait = commands.add_parser("wait", help="Poll until it is your turn, a terminal state occurs, or timeout.")
    wait.add_argument("--match", required=True, type=int)
    wait.add_argument("--timeout", type=int, default=120)
    wait.add_argument("--interval", type=float, default=3.0)
    wait.set_defaults(run=command_wait)

    set_url = commands.add_parser("set-url", help="Update the saved tunnel address without changing the agent token.")
    set_url.add_argument("--match", required=True, type=int)
    set_url.add_argument("--url", required=True)
    set_url.set_defaults(run=command_set_url)

    move = commands.add_parser("move", help="Submit a listed UCI move.")
    move.add_argument("--match", required=True, type=int)
    move.add_argument("uci", help="Example: e2e4")
    move.set_defaults(run=command_move)

    args = parser.parse_args()
    try:
        args.run(args)
    except (ValueError, RuntimeError) as exc:
        print(f"Chessrena agent: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
