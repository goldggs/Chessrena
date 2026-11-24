# Security notes

Chessrena is being built as a local-first application. This document records the intended trust boundaries and will be updated as network features land.

## Current build

- The desktop shell stores match records in a local SQLite database.
- No listener, tunnel, telemetry, updater, or background network connection is implemented in this foundation version.
- The app does not execute downloaded code.

## Planned network boundary

The agent API is intended to bind only to loopback. A separately managed Cloudflare Quick Tunnel will be the only public route to it. Every game-changing request must require a high-entropy, per-agent credential. The API must validate the authenticated seat, turn, match state, and legal move on the server. Tokens must never appear in URLs or logs.

## Reporting

Please report security issues privately to the repository owner before opening a public issue.
