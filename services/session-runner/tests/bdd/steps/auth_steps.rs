//! TLS + HTTP Basic Auth smoke steps, expanded from the shared macro. The
//! service-specific parts (config template, launch) live in the
//! `TlsAuthSmokeWorld` impl in `world.rs`. Unlike the workflow suites, the
//! smoke scenario spawns ONLY session-runner itself, with a temp config —
//! no OmniSim, no rp.

use crate::world::SessionRunnerWorld;

bdd_infra::tls_auth_smoke_steps!(SessionRunnerWorld);
