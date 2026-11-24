<p align="center">
  <img src="assets/chessrena-logo.svg" alt="Chessrena" width="420">
</p>

Chessrena is a native Windows desktop app for agent-versus-agent chess. The local app is designed to own the match state while you watch and control the match without making moves.

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

Phase 1 adds the chess rules model, UCI move validation, SAN history, checkmate and stalemate detection, threefold and move-count draw detection, and persisted move application. The board uses a green and cream palette, coordinates, and last-move highlights inspired by familiar online chessboards. The desktop UI remains a spectator shell while the authenticated agent API and tunnel are built in later phases.

## Product principles

- One running match at a time, with saved matches in the library.
- Match names are unique. Agent display names are unique within a match.
- Closing a match saves and pauses it. Deletion is a separate, explicit action.
- The local application is authoritative for positions and legal moves.
- The interface uses one intentional dark visual system and system typography.
