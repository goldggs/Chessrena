# Security notes

Chessrena keeps match state authoritative on the local desktop app. The API listens on loopback (`127.0.0.1:18764`); when `cloudflared` is available, Chessrena starts an account-free Cloudflare Quick Tunnel to that local API. The Quick Tunnel provides a temporary public HTTPS address and is stopped when Chessrena exits.

## Implemented boundaries

- The API does not bind to a LAN or public interface. Do not expose port `18764` directly.
- Each match has two separate invitation tokens with 256 bits of OS-generated randomness. SQLite stores SHA-256 hashes, never the raw invitation strings.
- API endpoints other than the non-sensitive health check require the matching seat's bearer token. Credentials are not placed in URLs.
- The app checks registration, readiness, assigned seat/color, active-match state, turn, position version, and legal moves. Remote agents cannot start, pause, finish, rename, delete, or assign colors.
- The Python helper uses only the standard library, blocks redirects, permits HTTPS for remote URLs, and allows plain HTTP only for loopback development. Config writes use a temporary file and restrictive permissions where supported.
- Chessrena launches `cloudflared` without a shell and terminates its process when the app exits. It accepts only an HTTPS `*.trycloudflare.com` URL from cloudflared output.

## Exposure and trust

- A Quick Tunnel URL is public and temporary. Anyone who discovers it can reach the API, but still needs a valid, high-entropy seat token for match operations. Treat both URL and invite prompts as sensitive; distribute prompts only to the intended agents.
- HTTPS terminates at Cloudflare. Cloudflare is therefore in the network path and can observe API traffic, including bearer credentials. The local origin leg from `cloudflared` to `127.0.0.1` is HTTP on the same machine.
- Quick Tunnels have no uptime guarantee and their hostname changes when recreated. The UI displays the current URL and warns when it changes; agents must update their saved endpoint using `set-url`.
- If `cloudflared` is missing, the explicit Download / retry secure link action downloads the official Windows x64 executable from [Cloudflare's GitHub release](https://github.com/cloudflare/cloudflared/releases/tag/2026.8.2) and verifies its SHA-256 before saving it under `%LOCALAPPDATA%\\Chessrena`. Current pin: release `2026.8.2`, SHA-256 `c29eee2b121f5436a642eed69fd9767da7e7b8c510fa50aaa130337f931357b5`. The pinned release must be updated deliberately as part of maintenance. No account, domain, or signup-based tunnel is required.

## Remaining risks / review items

- Per-IP rate limiting, request quotas, abuse controls, and audit logging are not implemented. A high-entropy token prevents guessing but does not mitigate traffic flooding.
- The Quick Tunnel is a convenience feature, not a production hosting or availability mechanism. Stop Chessrena to revoke its live URL; rotate a seat invitation in the app if its token is exposed.
- Windows release signing and independent security review remain outstanding.

Until rate limits and operational controls are added, use only for trusted casual matches. Report security issues privately to the repository owner before opening a public issue.
