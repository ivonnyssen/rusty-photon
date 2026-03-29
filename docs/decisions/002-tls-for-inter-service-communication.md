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

## ACME / Let's Encrypt Support

### Motivation

The self-signed CA approach requires every client to be configured with
the CA certificate. Third-party Alpaca devices with publicly-signed
certificates cannot be mixed with self-signed services in a single
sentinel configuration without a custom certificate verifier. Publicly-
trusted certificates from Let's Encrypt remove both limitations: browsers
and clients trust Let's Encrypt natively, and no manual CA installation
is needed.

### Why DNS-01 Is the Only Viable Challenge Type

Observatory services run on local networks behind NAT. They are not
reachable on ports 80 or 443 from the internet. Of the three ACME
challenge types, only DNS-01 works in this environment:

| Challenge    | Requires                  | Behind NAT? | Wildcards? |
|------------- |---------------------------|-------------|------------|
| HTTP-01      | Port 80 from internet     | No          | No         |
| TLS-ALPN-01  | Port 443 from internet    | No          | No         |
| **DNS-01**   | **DNS TXT record via API** | **Yes**     | **Yes**    |

DNS-01 proves domain ownership by creating a TXT record at
`_acme-challenge.<DOMAIN>`. Let's Encrypt validates via public DNS. The
actual services can live on `192.168.x.x` behind a home router — they
never need to be publicly accessible.

### User Experience

#### One-Time Setup

```bash
rp init-tls --acme \
  --domain observatory.example.com \
  --dns-provider cloudflare \
  --dns-token "$CLOUDFLARE_API_TOKEN" \
  --email user@example.com
```

This single command:

1. Creates an ACME account with Let's Encrypt
2. Requests a wildcard certificate for `*.observatory.example.com`
3. Creates a DNS TXT record via the Cloudflare API for validation
4. Waits for DNS propagation, completes the challenge
5. Writes the certificate chain and private key to
   `~/.rusty-photon/pki/certs/`
6. Prints configuration hints for each service

After this, services use publicly-trusted certificates. No CA
installation on clients. No manual renewal.

#### Dual Path (Self-Signed Remains Default)

Users without a domain (or on air-gapped networks) keep using the
self-signed CA. ACME is an opt-in alternative:

```bash
rp init-tls                                          # self-signed CA
rp init-tls --acme --domain observatory.example.com  # Let's Encrypt
```

#### Staging Environment

Let's Encrypt provides a staging endpoint with much higher rate limits
and untrusted root CAs. Users should test with staging before switching
to production:

```bash
rp init-tls --acme --staging --domain observatory.example.com ...
```

The `--staging` flag uses `acme-staging-v02.api.letsencrypt.org`. This
is the recommended first step to verify DNS provider credentials and
challenge flow without risking rate limit exhaustion.

### Single Wildcard Certificate

All services share one wildcard certificate
(`*.observatory.example.com`) rather than per-service certificates.

**Rationale:**

- One ACME order, one DNS challenge, one renewal — not five
- Let's Encrypt rate limits (50 certs/domain/week) are a non-issue
  with a single cert (renewals are exempt from rate limits)
- Each service is distinguishable by subdomain (e.g.,
  `filemonitor.observatory.example.com`,
  `rp.observatory.example.com`)

**Trade-off:** A compromised key exposes all services. This is
acceptable for single-machine deployment where all services share the
same trust boundary. For multi-machine setups, the same cert is
distributed to each machine — the trust boundary is the observatory
network, not individual hosts.

### Multi-Machine Support

The wildcard certificate is valid for all subdomains regardless of which
IP they resolve to. Services on different machines simply need a copy of
the same cert and key files:

```
*.observatory.example.com cert covers all of:

filemonitor.observatory.example.com  →  192.168.1.50  (Pi in the dome)
rp.observatory.example.com           →  192.168.1.50  (same Pi)
camera.observatory.example.com       →  192.168.1.51  (different machine)
mount.observatory.example.com        →  192.168.1.52  (yet another)
```

Users configure split-horizon DNS so domain names resolve to local IPs.
Common approaches: Pi-hole/dnsmasq overrides, local hosts file entries,
or Tailscale MagicDNS. The public DNS records exist only for ACME
challenge validation — they do not need to resolve to anything
meaningful externally.

Certificate distribution to remote machines is handled via configurable
`post_renewal_hooks` in the ACME config (see Configuration section).

### Configuration

ACME config is **standalone** at `~/.rusty-photon/acme.json`, not
embedded in any service config. This decouples certificate management
from any specific service and supports multi-machine deployments where
the ACME client runs on one host:

```json
{
  "email": "user@example.com",
  "domain": "observatory.example.com",
  "dns_provider": "cloudflare",
  "dns_credentials": {
    "api_token": "$CLOUDFLARE_API_TOKEN"
  },
  "staging": false,
  "renewal_days_before_expiry": 30,
  "post_renewal_hooks": [
    "scp ~/.rusty-photon/pki/certs/acme-*.pem pi-dome:~/.rusty-photon/pki/certs/"
  ]
}
```

| Field                         | Required | Description |
|-------------------------------|----------|-------------|
| `email`                       | Yes      | ACME account email for expiry notifications |
| `domain`                      | Yes      | Base domain (wildcard cert issued for `*.<domain>`) |
| `dns_provider`                | Yes      | DNS provider identifier (e.g., `"cloudflare"`) |
| `dns_credentials`             | Yes      | Provider-specific credentials; values starting with `$` are read from environment variables |
| `staging`                     | No       | Use Let's Encrypt staging endpoint (default: `false`) |
| `renewal_days_before_expiry`  | No       | Days before expiry to trigger renewal (default: `30`) |
| `post_renewal_hooks`          | No       | Shell commands to run after successful renewal (e.g., distribute certs to remote machines) |

#### File Layout

```
~/.rusty-photon/
├── acme.json                  # ACME configuration (standalone)
├── pki/
│   ├── acme-account.json      # ACME account credentials (persistent)
│   ├── ca.pem                 # Self-signed CA (unused with ACME)
│   ├── ca-key.pem
│   └── certs/
│       ├── acme-cert.pem      # Wildcard certificate chain
│       └── acme-key.pem       # Wildcard private key
```

#### Service Config (Unchanged Structure)

Each service still uses the existing `tls` section. With ACME, all
services point to the same wildcard cert files:

```json
{
  "server": {
    "port": 11112,
    "tls": {
      "cert": "~/.rusty-photon/pki/certs/acme-cert.pem",
      "key": "~/.rusty-photon/pki/certs/acme-key.pem"
    }
  }
}
```

On remote machines the paths are the same — certs arrive via
`post_renewal_hooks`.

### Pluggable DNS Providers

DNS provider interaction is behind a trait so new providers can be added
without touching ACME logic:

```rust
#[async_trait]
pub trait DnsProvider: Send + Sync + Debug {
    /// Create a TXT record for the ACME challenge.
    async fn create_txt_record(&self, fqdn: &str, value: &str) -> Result<()>;

    /// Remove the TXT record after validation completes.
    async fn delete_txt_record(&self, fqdn: &str) -> Result<()>;
}
```

The initial implementation ships **Cloudflare** only (free tier, widely
used for domain management). The Cloudflare implementation uses the
official [`cloudflare`](https://crates.io/crates/cloudflare) Rust crate,
which handles authentication, error responses, zone ID lookup, and
retry logic out of the box.

Adding a new provider means implementing the `DnsProvider` trait and
registering the provider identifier in the config parser. No changes to
ACME orchestration, certificate management, or CLI.

### Automatic Certificate Renewal

Let's Encrypt certificates currently have a 90-day lifetime, moving to
45 days by February 2028. Automated renewal is non-negotiable.

#### Renewal Flow

When `rp serve` starts and `~/.rusty-photon/acme.json` exists:

1. A background tokio task checks certificate expiry daily
2. If within `renewal_days_before_expiry` of expiration:
   a. Load ACME account from `~/.rusty-photon/pki/acme-account.json`
   b. Create a new ACME order via `instant-acme`
   c. Complete DNS-01 challenge via the configured `DnsProvider`
   d. Finalize the order and retrieve the new certificate chain
   e. Write the new cert and key to `~/.rusty-photon/pki/certs/`
   f. Hot-swap the certificate in the running server (see below)
   g. Execute `post_renewal_hooks` to distribute to remote machines

#### Manual Fallback

A `rp renew-tls` CLI command provides the same flow as a one-shot
operation, for users who prefer cron/systemd timers or want to trigger
renewal explicitly.

### Certificate Hot-Reloading

The current implementation uses `with_single_cert()` which bakes the
certificate into the `ServerConfig` at startup. For ACME, certificates
must be swappable without restarting services.

#### ResolvesServerCert Implementation

Replace `with_single_cert()` with a custom `ResolvesServerCert`
implementation backed by an `RwLock<Arc<CertifiedKey>>`:

```rust
#[derive(Debug)]
pub struct ReloadableCertResolver {
    current: RwLock<Arc<CertifiedKey>>,
}

impl ResolvesServerCert for ReloadableCertResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.read().unwrap().clone())
    }
}

impl ReloadableCertResolver {
    pub fn swap(&self, new_key: Arc<CertifiedKey>) {
        *self.current.write().unwrap() = new_key;
    }
}
```

In `server.rs`:

```rust
// Before:
.with_single_cert(certs, key)?

// After:
.with_cert_resolver(Arc::new(resolver))
```

This change is backward-compatible — self-signed certificates also use
the resolver; they simply never get swapped.

For remote machines receiving certs via `post_renewal_hooks`, services
either:

- Use a file-watcher (e.g., `tls-hot-reload` crate) to detect changes
  and call `resolver.swap()` automatically
- Restart on the next maintenance window — renewal happens at most once
  every ~60 days, so a brief restart is acceptable

### ACME Library: instant-acme

[`instant-acme`](https://crates.io/crates/instant-acme) (v0.8) is the
ACME protocol library. It is the only Rust crate supporting DNS-01
challenges and aligns with the project's existing dependency stack:

| Dependency   | instant-acme | Rusty Photon | Compatible? |
|------------- |------------- |------------- |-------------|
| `rcgen`      | Yes          | Yes (v0.13)  | Yes         |
| `tokio`      | Yes          | Yes          | Yes         |
| `rustls`     | Yes          | Yes (v0.23)  | Yes         |
| Crypto       | `aws-lc-rs`  | `aws-lc-rs`  | Yes         |

Features used: full RFC 8555 implementation (async), DNS-01 challenge
support, CSR generation via `rcgen`, serializable account credentials
for persistence, and certificate revocation.

### Client-Side Trust: No CA Configuration Needed

With the self-signed CA, clients (rp, sentinel) must be configured with
`ca_cert` and use `tls_certs_only()` in the reqwest builder to disable
platform root certificates. This workaround exists because the macOS
Security framework rejects self-signed CAs that are not in the system
keychain — the platform verifier and the custom CA conflict.

With Let's Encrypt, this problem disappears entirely. Let's Encrypt's
root CA is already in every platform's trust store (macOS, Windows,
Linux). Clients use the default reqwest builder with no `ca_cert`
configuration — the `build_reqwest_client(None)` path works on all
platforms without any special handling.

The `ca_cert` / `tls_certs_only()` code path remains for self-signed CA
users only.

### Security Considerations

| Concern                   | Mitigation |
|---------------------------|------------|
| DNS API token on disk     | `$ENV_VAR` syntax in config; file permissions restricted to 0600 |
| Private key storage       | Same `~/.rusty-photon/pki/` directory with 0600 permissions |
| ACME account key          | Separate from cert keys; stored in `acme-account.json` |
| Rate limit exhaustion     | Always test with `--staging` first; renewals are exempt from rate limits |
| Compromised cert key      | One wildcard cert = one key to protect; same trust boundary as self-signed |
| DNS credential blast radius | Optional CNAME delegation: `_acme-challenge.observatory.example.com CNAME _acme-challenge.acme.example.com` limits write access to a dedicated validation zone |

#### Let's Encrypt Rate Limits (Production)

| Limit                          | Value            |
|--------------------------------|------------------|
| Certificates per domain / week | 50 (renewals exempt) |
| Duplicate certificates / week  | 5                |
| Orders per account / 3 hours   | 300              |
| Auth failures per host / hour  | 5                |

For a single observatory with one wildcard cert, these limits are not a
practical concern.

### New Dependencies

```toml
# Workspace Cargo.toml [workspace.dependencies]
instant-acme = "0.8"
cloudflare = "0.12"
```

### Implementation Phases

#### Phase 1: Core ACME Infrastructure

- `DnsProvider` trait and Cloudflare implementation in `rp-tls`
- ACME account creation and persistence via `instant-acme`
- DNS-01 challenge solver (create record → wait for propagation →
  respond to challenge → clean up record)
- Certificate issuance flow (order → challenge → finalize → download)
- Extend `rp init-tls` CLI with `--acme` flags

#### Phase 2: Certificate Hot-Reloading

- `ReloadableCertResolver` in `rp-tls`
- Modify `server.rs` to use `with_cert_resolver` (backward-compatible)
- Background renewal task in `rp serve`
- `rp renew-tls` manual command

#### Phase 3: Polish and Testing

- Staging/production toggle (`--staging`)
- Configuration validation and helpful error messages
- BDD tests using [Pebble](https://github.com/letsencrypt/pebble)
  (Let's Encrypt's official ACME test server) for end-to-end flows
- `post_renewal_hooks` execution
- Documentation updates

### Future: DNS-PERSIST-01

A new challenge type approved by the CA/Browser Forum (October 2025).
Instead of creating a new TXT record for each renewal, the user sets a
persistent authorization record once:

```
_acme-challenge.observatory.example.com TXT "acme-persist=<value>"
```

This eliminates DNS provider API credentials from the renewal flow
entirely — credentials are needed only during initial setup.

Expected production availability: Q2 2026. When available, it would be
added as an alternative challenge strategy behind the same `DnsProvider`
trait, dramatically simplifying the ongoing user experience.

## References

- [rcgen crate](https://crates.io/crates/rcgen) — pure Rust X.509
  certificate generation
- [rustls](https://crates.io/crates/rustls) — modern TLS in Rust
- [instant-acme](https://crates.io/crates/instant-acme) — pure Rust
  ACME client for Let's Encrypt / ZeroSSL
- [Let's Encrypt challenge types](https://letsencrypt.org/docs/challenge-types/)
- [Let's Encrypt rate limits](https://letsencrypt.org/docs/rate-limits/)
- [Let's Encrypt staging environment](https://letsencrypt.org/docs/staging-environment/)
- [Let's Encrypt certificate lifetime changes](https://letsencrypt.org/2025/12/02/from-90-to-45)
- [DNS-PERSIST-01 announcement](https://letsencrypt.org/2026/02/18/dns-persist-01)
- [Pebble ACME test server](https://github.com/letsencrypt/pebble)
- [rustls ResolvesServerCert](https://docs.rs/rustls/latest/rustls/server/trait.ResolvesServerCert.html)
- [ASCOM Alpaca API](https://ascom-standards.org/api/)
- [axum TLS examples](https://github.com/tokio-rs/axum/tree/main/examples/tls-rustls)
