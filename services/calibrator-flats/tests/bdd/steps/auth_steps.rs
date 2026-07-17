//! TLS + HTTP Basic Auth smoke steps, expanded from the shared macro. The
//! service-specific parts (config template, launch) live in the
//! `TlsAuthSmokeWorld` impl in `world.rs`. Unlike the workflow suite, the
//! smoke scenario spawns ONLY calibrator-flats itself, with a temp config —
//! no OmniSim, no rp.

use crate::world::CalibratorFlatsWorld;

bdd_infra::tls_auth_smoke_steps!(CalibratorFlatsWorld);
