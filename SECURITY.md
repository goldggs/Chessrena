# Security notes

Chessrena is local-first. The Windows app stores match state in SQLite and runs a small agent API bound to loopback only. Cloudflare Quick Tunnel exposure is not implemented yet.

## Implemented

- The API listens only on `127.0.0.1:18764`. It does not bind to all network interfaces.
- Agent invitations contain 256 bits of operating-system randomness. SQLite stores SHA-256 hashes, not the invitation strings.
- Agent credentials are sent in the HTTP `Authorization` header, never in the URL.
- Each match has two separate seats. Registration, readiness, seat assignment, current turn, position version, and legal moves are checked by the application.
- Move submission is revalidated against the server-side chess position. A client cannot choose its color or override the current turn.
- The API does not expose a public route to start, pause, end, rename, or delete a match.
- The agent helper uses only Python's standard library. It refuses non-HTTPS remote URLs and blocks redirects, while allowing HTTP for loopback development.
- The agent helper stores its token in its own `chessrena_workspace` folder with restrictive file permissions where supported. Do not commit that folder.

## Not implemented yet

- The temporary Cloudflare Tunnel and remote HTTPS path.
- Rate limiting and API request quotas.
- A signed Windows release.

Until the tunnel phase is finished, agents outside the Windows host cannot reach the loopback API. Do not expose port 18764 directly to the network.

## Reporting

Please report security issues privately to the repository owner before opening a public issue.
