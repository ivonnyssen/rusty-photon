# ADR-003: Authentication for Device Access

## Status

Accepted

## Updates

**2026-04-22** — Upstream `ascom-alpaca-rs` replaced
`Server::into_router() -> axum::Router` with `Server::into_service() ->
AlpacaService`. The code snippets below still illustrate the
integration point; in current code each service wraps the returned
service with `Router::new().fallback_service(server.into_service())`
before calling `rp_auth::layer(router, auth)`. The authentication
decision is unchanged — only the upstream adapter shape changed. See
`services/*/src/lib.rs` for the live pattern.

## Context

ADR-002 introduced opt-in TLS for inter-service communication, protecting
traffic confidentiality and integrity. However, TLS alone does not answer
the question "who is allowed to talk to this service?" Any client that
can reach the port — on the local network or over the internet — can
issue equipment commands, read sensor data, and control observatory
sessions without restriction.

The ASCOM Alpaca specification explicitly declares no security mechanisms
(`security: []` in both the Device API and Management API OpenAPI
definitions). The design philosophy is that "Alpaca and network security
are separate things." The standard relies on network isolation (private
observatory LANs, NAT routers) as the primary security model.

This works for a single-user home observatory on a dedicated network,
but breaks down in increasingly common scenarios:

- **Remote observatories** accessed over VPN or the internet
- **Shared club networks** with multiple users and devices
- **Mixed networks** where observatory equipment shares Wi-Fi with other
  household or facility devices

The goal is to add opt-in authentication that:

1. Ensures only authorized users can access Alpaca devices
2. Is easy to configure for hobbyist astronomers
3. Uses a common, straightforward scheme that other manufacturers can
   adopt
4. Works for both local and remote device access
5. Does not require fine-grained scopes or role-based access — just
   "authorized or not"

## Options Considered

### Option 1: HTTP Basic Auth over TLS (Chosen)

The client sends `Authorization: Basic <base64(username:password)>` with
every request (RFC 7617). The server validates the credentials and
returns `401 Unauthorized` with `WWW-Authenticate: Basic realm="..."` on
failure.

**Pros:**
- The only scheme already supported by the ASCOM ecosystem — ASCOM
  Dynamic Clients (NINA, SGPro, Windows Platform) have native
  username/password fields in their Alpaca setup dialogues
- The ASCOM OmniSim reference server implements exactly this: HTTP Basic
  Auth with PBKDF2 password hashing, opt-in, off by default
- Trivial to implement — ~20 lines of axum tower middleware
- Trivial to configure — users understand username/password pairs
- Trivial for other manufacturers — every HTTP framework has built-in
  support
- Adequate security over TLS — credentials encrypted in transit, replay
  and tampering prevented at the transport layer
- Same model used by Home Assistant REST API, Tasmota, OctoPrint, NAS
  devices, and router web UIs

**Cons:**
- Credentials are Base64-encoded (trivially reversible), not encrypted at
  the application layer — requires TLS for security
- No built-in expiration or rotation mechanism
- No per-client revocation — changing the password affects all clients
- Sends credentials with every request (no session/token caching)

### Option 2: API Keys (Custom Header)

The server generates a random string. The client sends it as
`X-Api-Key: <key>` or `Authorization: Bearer <key>`.

**Pros:**
- Simple to implement (~30 lines of middleware)
- No username/password semantics — a single opaque token
- Easy revocation — generate a new key and the old one is invalid
- Per-client keys enable selective revocation
- Established pattern in IoT (OctoPrint, Philips Hue, Home Assistant)

**Cons:**
- Not a standard HTTP auth mechanism — no `WWW-Authenticate` challenge,
  custom header names vary across implementations
- No existing ASCOM Alpaca client supports API key headers — breaking
  change for the ecosystem
- Static credentials — no expiration unless manually rotated
- Requires TLS, same as Basic Auth

### Option 3: HTTP Digest Auth (RFC 7616)

Challenge-response: the server sends a nonce, the client hashes the
credentials with the nonce and sends the hash.

**Pros:**
- Password never sent in cleartext, even without TLS
- Nonce-based replay protection

**Cons:**
- Largely deprecated — NIST/CISA guidance favors Basic Auth over TLS
- Complex implementation (~200 lines, nonce management, qop handling)
- `reqwest` (our HTTP client) does not support Digest natively
- No advantage over Basic Auth when TLS is available
- Not used by any ASCOM Alpaca implementation
- The RFC itself acknowledges: "For those needs, TLS is a more
  appropriate protocol"

### Option 4: Bearer Tokens / JWT (RFC 6750 / RFC 7519)

Signed JSON tokens with expiration, issuer claims, etc.

**Pros:**
- Stateless validation via cryptographic signature
- Built-in expiration (`exp` claim)
- Rich metadata for distributed systems

**Cons:**
- Overkill for "authorized or not" on a single device server
- Signing key management adds complexity
- No revocation without a revocation list (defeating "stateless")
- Token generation requires a login endpoint or pre-generation
- No ASCOM Alpaca client support
- Solves distributed identity problems that don't exist here

### Option 5: Mutual TLS / mTLS (RFC 8705)

Both client and server present X.509 certificates during the TLS
handshake.

**Pros:**
- Strongest authentication — cryptographic client identity
- No credentials in the HTTP layer
- Leverages existing PKI from ADR-002

**Cons:**
- Generating, distributing, and installing client certificates is a
  significant usability barrier for hobbyist astronomers
- No existing ASCOM Alpaca client supports client certificates
- Certificate lifecycle management adds operational burden
- Used in industrial IoT (AWS IoT Core, Azure IoT Hub), not consumer
  or hobbyist device control

### Option 6: OAuth 2.0 (RFC 6749)

Authorization server issues tokens via grant flows.

**Pros:**
- Industry-standard framework
- Built-in token lifecycle management
- Separation of auth server and resource server

**Cons:**
- Requires a separate authorization server — running OAuth on a
  Raspberry Pi for a single-user observatory is unreasonable
- No ASCOM Alpaca client support
- Dramatically overengineered for this use case

### Option 7: HMAC Signing (AWS SigV4 style)

Each request is signed using HMAC-SHA256. The signature covers the HTTP
method, URL, headers, timestamp, and body.

**Pros:**
- Request integrity — tampering detected even beyond TLS
- Replay protection via timestamps
- Secret never transmitted

**Cons:**
- ~400 lines of server code, ~200–300 lines per client language
- Fragile canonicalization — minor differences cause signature mismatches
- Clock synchronization required — problematic for observatory setups
  without NTP
- Redundant with TLS integrity and replay protection
- Prohibitive implementation burden for other manufacturers

## Decision

We chose **Option 1: HTTP Basic Auth over TLS (RFC 7617)**.

The decisive factor is ecosystem compatibility. ASCOM Alpaca clients
already have native username/password fields, and the ASCOM OmniSim
reference server validates this exact approach. Every other option would
require changes to third-party client software that we do not control.

Basic Auth over TLS provides adequate security for this threat model:
credentials are encrypted in transit, the TLS channel prevents replay and
tampering, and the residual risk is credential management — the same
tradeoff accepted by Home Assistant, OctoPrint, and every router web UI.

Like TLS (ADR-002), authentication is **opt-in and off by default**.
Services without an `auth` configuration section run unauthenticated,
preserving backward compatibility.

## Implementation

### Credential Storage

Credentials are stored in each service's configuration file. Passwords
are hashed using Argon2id (the current OWASP recommendation for password
hashing):

```toml
[server.auth]
username = "observatory"
password_hash = "$argon2id$v=19$m=19456,t=2,p=1$..."
```

A CLI command generates the hash from a plaintext password:

```bash
rp hash-password
# Enter password: ********
# Confirm password: ********
# $argon2id$v=19$m=19456,t=2,p=1$...
```

The user pastes the output into their service config. This avoids storing
plaintext passwords in configuration files.

**Why Argon2id over PBKDF2:**
The ASCOM OmniSim uses PBKDF2 (RFC 2898, 1000 iterations). Argon2id
(RFC 9106, winner of the Password Hashing Competition) is the current
OWASP recommendation. It is memory-hard, resisting GPU/ASIC attacks that
PBKDF2 is vulnerable to. The `argon2` crate is pure Rust with no system
dependencies.

### Authentication Middleware

A tower middleware layer validates the `Authorization: Basic` header on
every request. The middleware is added to the axum router extracted from
`ascom-alpaca-rs`'s `Server::into_router()`, the same integration point
used for TLS (ADR-002).

```rust
// Pseudocode — actual implementation in rp-auth crate
async fn auth_middleware(
    State(credentials): State<Credentials>,
    request: Request,
    next: Next,
) -> Response {
    match extract_basic_auth(&request) {
        Some((user, pass)) if credentials.verify(user, pass) => {
            next.run(request).await
        }
        Some(_) => unauthorized_response(),   // 401 — wrong credentials
        None    => unauthorized_response(),   // 401 — missing header
    }
}

fn unauthorized_response() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("WWW-Authenticate", "Basic realm=\"Rusty Photon\"")
        .body(Body::empty())
        .unwrap()
}
```

The `WWW-Authenticate` header is required by RFC 7235 and triggers
browser credential prompts (useful for accessing the sentinel dashboard).

### Shared Crate: rp-auth

Authentication logic lives in a new workspace crate, `crates/rp-auth`,
following the same pattern as `crates/rp-tls`:

- `credentials.rs` — Argon2id hashing and verification
- `middleware.rs` — axum/tower authentication layer
- `config.rs` — `AuthConfig` struct (username, password_hash)

This avoids duplicating auth logic across services.

### Service Config Changes

Each service gains an optional `auth` section nested under `server`:

```json
{
  "server": {
    "port": 11112,
    "tls": {
      "cert": "~/.rusty-photon/pki/certs/ppba-driver.pem",
      "key": "~/.rusty-photon/pki/certs/ppba-driver-key.pem"
    },
    "auth": {
      "username": "observatory",
      "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$..."
    }
  }
}
```

When `auth` is absent or null, the service runs without authentication.
Authentication is opt-in and non-breaking.

### Server Startup (per service)

The router wrapping extends the existing TLS branch from ADR-002:

```rust
let router = server.into_router();

// Layer authentication if configured
let router = match &config.server.auth {
    Some(auth) => rp_auth::layer(router, auth),
    None       => router,
};

// Bind and serve (TLS or plain)
let listener = bind_dual_stack(addr)?;
match &config.server.tls {
    Some(tls) => serve_with_tls(listener, router, tls).await,
    None      => serve_plain(listener, router).await,
}
```

Authentication is applied before TLS wrapping — the middleware operates
at the HTTP layer regardless of whether the transport is encrypted.

### Client-Side Configuration

`rp` and `sentinel` (which are HTTP clients of other services) gain
optional auth configuration per target service:

```json
{
  "services": {
    "filemonitor": {
      "url": "https://localhost:11111",
      "auth": {
        "username": "observatory",
        "password": "my-secret-password"
      }
    }
  }
}
```

Client-side passwords are stored in plaintext in the config file (the
client needs the actual password to send Basic Auth headers). This is the
same model used by the ASCOM .NET library's `AlpacaConfiguration`.
File permissions (`chmod 600`) are the recommended protection.

### Discovery Server

The Alpaca discovery server (UDP multicast, port 32227) is unaffected.
Discovery only advertises the service port; it carries no credentials or
sensitive data. Clients discover the port, then authenticate when making
HTTP requests.

### Auth + TLS Interaction

Authentication without TLS sends credentials in cleartext over the
network. While the implementation does not prevent this combination (a
user might have other transport security such as a VPN), a startup
warning is logged:

```
WARN: Authentication is enabled but TLS is not. Credentials will be
      transmitted in cleartext. Consider enabling TLS (see `rp init-tls`).
```

### Password Recovery

If a user forgets their password, they edit the service config file
directly — remove the `auth` section or replace the `password_hash`
with a new value from `rp hash-password`. No separate recovery mechanism
is needed.

## Consequences

### What Changes

- New workspace crate: `crates/rp-auth` (Argon2id hashing, tower
  middleware, config)
- `rp` gains a `hash-password` subcommand
- Each service's `ServerBuilder` gains auth middleware wrapping (~10
  lines)
- Service configs gain an optional `auth` section
- Client configs (`rp`, `sentinel`) gain optional per-service auth
  credentials
- New workspace dependency: `argon2` crate (pure Rust)

### What Doesn't Change

- Plain HTTP without auth remains the default — no existing setup breaks
- TLS infrastructure (ADR-002) is unchanged
- The Alpaca protocol itself is unchanged
- The discovery protocol is unchanged
- Third-party Alpaca devices are accessed using whatever auth their
  client supports
- No scopes, roles, or fine-grained access control

### User Experience

```bash
# One-time setup
rp hash-password
# Enter password: ********
# $argon2id$v=19$m=19456,t=2,p=1$...

# Paste hash into service config under [server.auth]
# Enter username/password in NINA, SGPro, or other Alpaca client
```

### Security Properties

| Property              | Without TLS        | With TLS           |
|-----------------------|--------------------|--------------------|
| Credential secrecy    | None (cleartext)   | Yes (encrypted)    |
| Replay protection     | None               | Yes (TLS session)  |
| Tampering protection  | None               | Yes (TLS integrity) |
| Credential storage    | Argon2id hash      | Argon2id hash      |
| Brute-force resistance| Argon2id cost      | Argon2id cost      |

## Future: API Keys as a Secondary Mechanism

A future enhancement could add API key support alongside Basic Auth.
This would address scenarios where machine-to-machine tokens are more
convenient than username/password pairs:

- **Automation scripts** that should not embed plaintext passwords
- **Per-client revocation** — revoke one key without changing the
  password for all clients
- **Third-party integrations** where sharing a personal password is
  undesirable

The implementation would accept either `Authorization: Basic` or
`Authorization: Bearer <key>` (or `X-Api-Key: <key>`), with keys
generated via `rp generate-api-key` and stored as Argon2id hashes in the
service config. This matches the dual-auth model used by OctoPrint
(username/password + API keys) and Home Assistant (OAuth + long-lived
tokens).

This is explicitly deferred — Basic Auth alone satisfies all current
requirements and is the only mechanism compatible with existing ASCOM
Alpaca clients.

## Future: Rate Limiting

Authentication opens the door for brute-force attacks against the
password. A future enhancement could add per-IP rate limiting on failed
authentication attempts (e.g., exponential backoff after 5 failures).
This is a defense-in-depth measure; Argon2id's computational cost
already makes online brute-force impractical for reasonable passwords.

## References

- [RFC 7617 — The 'Basic' HTTP Authentication Scheme](https://www.rfc-editor.org/rfc/rfc7617.html)
- [RFC 7235 — HTTP/1.1 Authentication](https://www.rfc-editor.org/rfc/rfc7235.html)
- [RFC 9106 — Argon2 Memory-Hard Function](https://www.rfc-editor.org/rfc/rfc9106.html)
- [OWASP Password Storage Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)
- [ASCOM Alpaca API — security: \[\]](https://ascom-standards.org/api/)
- [ASCOM.Alpaca.Simulators — AuthorizationFilter](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators)
- [ASCOM Library — AlpacaConfiguration](https://github.com/ASCOMInitiative/ASCOMLibrary)
- [argon2 crate](https://crates.io/crates/argon2) — pure Rust Argon2id
- [axum-auth crate](https://crates.io/crates/axum-auth) — Basic/Bearer
  extractors for axum
- [ADR-002 — TLS for Inter-Service Communication](./002-tls-for-inter-service-communication.md)
