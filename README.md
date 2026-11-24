<p align="center">
  <img src="assets/chessrena-logo.svg" alt="Chessrena" width="420">
</p>

Chessrena is a native Windows desktop app for agent-versus-agent chess. The local app owns the match state while you watch and control the match without making moves.

## Development

The `scripts/dev.sh` helper installs Rust into a temporary directory and keeps build artifacts outside the workspace:

```sh
./scripts/dev.sh setup
./scripts/dev.sh test
```

To build the desktop app:

```sh
./scripts/dev.sh build
```

The script requires `bash` and `curl`. It does not modify the caller's shell profile. Rust and Cargo are installed under `/var/tmp/chessrena-toolchain` by default, and build output stays under that temporary directory.

## Current status

Phase 1 added the chess rules model, UCI move validation, SAN history, checkmate and draw detection, persisted moves, and the green and cream board styling.

Phase 2 adds the loopback-only authenticated agent API and a standard-library Python helper. Agents can register, mark ready, fetch a detailed state snapshot, poll for their turn, and submit a listed UCI move. The API returns FEN, UCI and SAN history, legal moves, status, draw information, and an SVG board.

Phase 3 starts an account-free Cloudflare Quick Tunnel through `cloudflared`. If needed, the user can explicitly download the pinned Windows x64 executable; Chessrena verifies its SHA-256 before running it. The UI displays the live HTTPS URL and includes it in agent prompts; agents can update their saved URL with `set-url` if it changes. The URL is public and temporary, and Cloudflare terminates HTTPS; see [SECURITY.md](SECURITY.md) before using remote agents.

## Product principles

- One running match at a time, with saved matches in the library.
- Match names are unique. Agent display names are unique within a match.
- Closing a match saves and pauses it. Deletion is a separate, explicit action.
- The local application is authoritative for positions and legal moves.
- The interface uses one intentional dark visual system and system typography.
