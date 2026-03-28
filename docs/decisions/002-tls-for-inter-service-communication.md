# ADR-002: TLS for Inter-Service Communication

## Status

Proposed

## Context

All Rusty Photon services communicate over plain HTTP. ASCOM Alpaca is an
HTTP-based protocol with no built-in security. On a typical observatory
network (Raspberry Pi, local Wi-Fi), traffic between services —
including equipment commands, sensor readings, and session state — is
unencrypted and unauthenticated.

While the threat model for an isolated observatory network is modest,
unsecured HTTP means any device on the network can observe or inject
commands. As remote observatory setups become more common (VPN access
over the internet, shared club networks), the risk grows.

The goal is to enable HTTPS between services we control, without
requiring users to manage certificates manually, and without forcing TLS
on third-party Alpaca devices that don't support it.

## Options Considered

### Option 1: TLS Support Inside ascom-alpaca-rs Crate

Add a `tls` feature flag to the upstream `ascom-alpaca` crate with
`rustls` and `tokio-rustls` dependencies. The crate's `Server::bind()`
would accept an optional `TlsConfig` and wrap the listener internally.

**Pros:**
- All Alpaca servers get TLS automatically
- Single implementation shared across all services

**Cons:**
- Adds TLS dependencies to a general-purpose ASCOM crate that many
  consumers may not need
- Couples TLS policy (cert paths, cipher suites) to the upstream crate
- Harder to maintain — TLS changes require upstream releases
- The crate currently has zero TLS awareness; this is a significant
  scope expansion for a library focused on the Alpaca protocol

### Option 2: Expose Router, Handle TLS in Rusty Photon (Chosen)

Make `ascom-alpaca`'s `Server::into_router()` method public (currently
private). Rusty Photon services extract the `axum::Router`, handle
socket binding and optional TLS wrapping themselves.

**Pros:**
- Minimal upstream change — one visibility modifier (`fn` to `pub fn`)
- All TLS logic lives in Rusty Photon where it belongs
- Rusty Photon controls cert paths, cipher config, and the
  TLS-vs-plain-HTTP decision
- The `ascom-alpaca` crate stays focused on protocol correctness
- Services that don't need TLS are unaffected

**Cons:**
- Rusty Photon must replicate the socket2 dual-stack binding logic from
  `ascom-alpaca` (roughly 15 lines)
- Discovery server must be started separately (already public API)
- Each rusty-photon service's `ServerBuilder` needs a small TLS branch

### Option 3: axum-server with rustls in Rusty Photon (No Upstream Change)

Use the `axum-server` crate to replace the entire server lifecycle,
bypassing `ascom-alpaca`'s bind/start entirely. Build the Alpaca routes
manually or via a hypothetical device-registration-only API.

**Pros:**
- Zero upstream changes

**Cons:**
- Requires duplicating or reverse-engineering all Alpaca route
  registration, management endpoints, and the setup page
- Fragile — breaks when ascom-alpaca adds new routes or changes
  internal structure
- Defeats the purpose of using the crate

### Option 4: TLS Termination Proxy (e.g., Caddy, nginx)

Run a reverse proxy in front of each service that terminates TLS and
forwards plain HTTP to the service.

**Pros:**
- Zero code changes to any service
- Battle-tested TLS implementations

**Cons:**
- Additional process per service (6+ proxies on a Raspberry Pi)
- Significant memory and CPU overhead on constrained hardware
- Complex deployment — users must configure and maintain proxy configs
- Adds latency to every request
- Contradicts the "minimal footprint" tenet

## Decision

We chose **Option 2: Expose Router, Handle TLS in Rusty Photon**.

The upstream change is minimal — making one method public. All TLS
complexity stays in our codebase where we can iterate on it freely. This
keeps `ascom-alpaca-rs` focused on protocol correctness and avoids
burdening the upstream crate with security policy decisions.

## Implementation

### Upstream Change (ascom-alpaca-rs)

Make `Server::into_router()` public. This is the only required change.
The discovery server is already public (`DiscoveryServer`,
`BoundDiscoveryServer`).

### Certificate Management

A new `rp init-tls` subcommand generates all certificates on first run:

1. **Root CA** — a self-signed CA certificate ("Rusty Photon Observatory
   CA"), valid for 10 years. Stored in `~/.rusty-photon/pki/ca.pem` and
   `ca-key.pem`.
2. **Per-service certificates** — one cert per configured service,
   signed by the CA. SANs include `localhost`, `127.0.0.1`, the system
   hostname, and any IPs from service configs. Stored in
   `~/.rusty-photon/pki/certs/{service-name}.pem` and
   `{service-name}-key.pem`.

Certificate generation uses the `rcgen` crate (pure Rust, no OpenSSL
dependency), keeping the build simple on all platforms including
Raspberry Pi.

Long-lived certificates (10 years) are appropriate because:
- This is a private network with a single operator
- There is no revocation infrastructure to maintain
- Observatory setups are not frequently reprovisioned

### Service Config Changes

Each service gains an optional `tls` section:

```json
{
  "server": {
    "port": 11112,
    "tls": {
      "cert": "~/.rusty-photon/pki/certs/ppba-driver.pem",
      "key": "~/.rusty-photon/pki/certs/ppba-driver-key.pem"
    }
  }
}
```

When `tls` is absent or null, the service runs plain HTTP as before.
TLS is opt-in and non-breaking.

### Server Startup (per service)

Each service's `ServerBuilder` gains a TLS branch:

```rust
let router = server.into_router();
let listener = bind_dual_stack(addr)?; // replicates socket2 logic

match &config.server.tls {
    Some(tls) => serve_with_tls(listener, router, tls).await,
    None      => axum::serve(listener, router.into_make_service()).await,
}
```

The dual-stack binding logic (~15 lines of socket2 code) is extracted
into a shared utility in the workspace.

### Client-Side Trust (rp, sentinel)

`rp` and `sentinel` are Alpaca HTTP clients. When a CA cert is
configured, they add it to the `reqwest` client:

```rust
let client = reqwest::Client::builder()
    .add_root_certificate(ca_cert)
    .build()?;
```

URLs in config use `https://` for TLS-enabled services and `http://`
for third-party Alpaca devices. Both work — the client follows the
scheme in the URL.

### Discovery Server

The Alpaca discovery server (UDP multicast, port 32227) is unaffected.
It advertises the service port; clients then connect over HTTPS if
configured. Discovery itself does not carry sensitive data.

## Consequences

### What Changes

- `ascom-alpaca-rs`: one method becomes public (`into_router`)
- Each service's `ServerBuilder` gains ~20 lines of TLS branching
- A shared `bind_dual_stack()` utility is added to the workspace
- `rp` gains an `init-tls` subcommand
- Service configs gain an optional `tls` section
- New workspace dependencies: `rcgen`, `rustls`, `tokio-rustls`,
  `rustls-pemfile`

### What Doesn't Change

- Plain HTTP remains the default — no existing setup breaks
- Third-party Alpaca devices (cameras, mounts) are accessed over
  whatever scheme their URL specifies
- The Alpaca protocol itself is unchanged
- The discovery protocol is unchanged
- Service-to-service communication patterns are unchanged

### User Experience

```bash
# One-time setup
rp init-tls

# That's it. Services read certs from config.
# To go back to plain HTTP, remove the tls section from configs.
```

### Platform Support

- `rcgen` and `rustls` are pure Rust — no system OpenSSL dependency
- Works on Linux (x86_64, ARM64), macOS, and Windows
- No impact on Raspberry Pi builds

## Future: ACME / Let's Encrypt Support

A future enhancement could add support for publicly-trusted certificates
via the ACME protocol (Let's Encrypt), eliminating the need for a
self-signed CA entirely.

### Motivation

The self-signed CA approach requires every client to be configured with
the CA certificate. Third-party Alpaca devices with publicly-signed
certificates cannot be mixed with self-signed services in a single
sentinel configuration without a custom certificate verifier. Publicly-
trusted certificates from Let's Encrypt would remove both limitations.

### How It Would Work

Let's Encrypt's **DNS-01 challenge** validates domain ownership by
checking a DNS TXT record — the services never need to be publicly
accessible. A user who owns a domain (e.g., `vonnyssen.com`) could:

1. Point subdomains to their local observatory IP:
   ```
   filemonitor.observatory.vonnyssen.com  →  192.168.1.100
   rp.observatory.vonnyssen.com           →  192.168.1.100
   ```
2. Run `rp init-tls --acme --domain observatory.vonnyssen.com`
3. Rusty Photon creates a DNS TXT record via the provider's API,
   validates with Let's Encrypt, and receives a wildcard certificate
   (`*.observatory.vonnyssen.com`) trusted by all clients natively.

### Implementation Sketch

- **ACME client**: `instant-acme` crate (v0.8, pure Rust, async/tokio,
  supports DNS-01, automatic CSR via `rcgen`). Uses the `aws-lc-rs`
  crypto backend, consistent with the rest of the project.
- **DNS provider**: Start with Cloudflare (free tier, simple REST API
  for TXT record management). Extensible to Route53, Google Cloud DNS.
- **Renewal**: Certificates expire every 90 days. A `rp renew-tls`
  command plus an optional systemd timer or cron job handles renewal.
- **CLI**: `rp init-tls --acme --domain <DOMAIN> --dns-provider
  cloudflare --dns-token <TOKEN>`
- **Account state**: ACME credentials stored in
  `~/.rusty-photon/acme/`.

### Dual Path

The self-signed CA (`rp init-tls`) remains the default for users
without a domain. ACME is an opt-in alternative for users who want
publicly-trusted certificates.

```bash
rp init-tls                                          # self-signed CA
rp init-tls --acme --domain observatory.example.com  # Let's Encrypt
```

## References

- [rcgen crate](https://crates.io/crates/rcgen) — pure Rust X.509
  certificate generation
- [rustls](https://crates.io/crates/rustls) — modern TLS in Rust
- [instant-acme](https://crates.io/crates/instant-acme) — pure Rust
  ACME client for Let's Encrypt / ZeroSSL
- [Let's Encrypt DNS-01 challenge](https://letsencrypt.org/docs/challenge-types/#dns-01-challenge)
- [ASCOM Alpaca API](https://ascom-standards.org/api/)
- [axum TLS examples](https://github.com/tokio-rs/axum/tree/main/examples/tls-rustls)
