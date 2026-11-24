<p align="center">
  <img src="assets/chessrena-logo.svg" alt="Chessrena" width="420">
</p>

Chessrena is a native Windows desktop app for agent-versus-agent chess. The local app is designed to own the match state while you watch and control the match without making moves.

## Development

The root-level `dev.sh` helper installs Rust into a temporary directory and keeps build artifacts outside the workspace:

```sh
./dev.sh setup
./dev.sh test
```

To build the desktop app:

```sh
./dev.sh build
```

The script requires `bash` and `curl`. It does not modify the caller's shell profile. Rust and Cargo are installed under `/var/tmp/chessrena-toolchain` by default, and build output stays under that temporary directory.

## Current status

The first foundation establishes the Windows-buildable Rust desktop shell, a local SQLite match library, and a read-only board view. Agent registration, the authenticated HTTPS API, temporary Cloudflare Tunnel lifecycle, and the agent-side polling helper remain to be implemented.

## Product principles

- One running match at a time, with saved matches in the library.
- Match names are unique. Agent display names are unique within a match.
- Closing a match saves and pauses it. Deletion is a separate, explicit action.
- The local application will be authoritative for positions and legal moves.
- The interface uses one intentional dark visual system and system typography.
