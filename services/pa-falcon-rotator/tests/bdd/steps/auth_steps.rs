//! TLS + HTTP Basic Auth smoke steps, expanded from the shared macro. The
//! service-specific parts (config template, in-process server launch with
//! the mock serial factory) live in the `TlsAuthSmokeWorld` impl in
//! `world.rs`.

use crate::world::FalconRotatorWorld;

bdd_infra::tls_auth_smoke_steps!(FalconRotatorWorld);
