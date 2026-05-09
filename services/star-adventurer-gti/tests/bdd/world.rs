//! World struct for star-adventurer-gti BDD tests.
//!
//! Phase 2 scaffold — all scenarios are tagged `@wip` so the helper bodies
//! below are not actually exercised yet. Phase 3 fills them in (spawn the
//! service binary, build a config, drive it through the Alpaca client).

#![allow(dead_code)] // Phase 3 wires every field

use std::sync::Arc;

use ascom_alpaca::api::telescope::Telescope;
use ascom_alpaca::ASCOMError;
use bdd_infra::ServiceHandle;
use cucumber::World;
use star_adventurer_gti::Config;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
pub struct StarAdventurerWorld {
    pub service_handle: Option<ServiceHandle>,
    pub mount: Option<Arc<dyn Telescope>>,
    pub config: Option<Config>,
    pub temp_dir: Option<TempDir>,
    pub last_error: Option<String>,
    pub last_error_code: Option<u16>,
}

impl StarAdventurerWorld {
    /// Convenience accessor — Phase 3 connects the dyn Telescope handle.
    pub fn mount(&self) -> &Arc<dyn Telescope> {
        self.mount
            .as_ref()
            .expect("mount client not acquired — did the service start?")
    }

    /// Phase 3 will: build a JSON config from `self.config`, write it to
    /// `temp_dir`, spawn the service via `ServiceHandle::start`, and poll
    /// the Alpaca client until the Telescope device is exposed.
    pub async fn start_service(&mut self) {
        todo!("Phase 3: spawn star-adventurer-gti binary against the mock transport")
    }

    /// Clear both `last_error` and `last_error_code` so a previous failure in
    /// the same scenario can't leak into later assertions. Used by the
    /// `try_*` step bodies on the success branch.
    pub fn clear_error(&mut self) {
        self.last_error = None;
        self.last_error_code = None;
    }

    /// Capture an [`ASCOMError`] returned by a `try_*` step into the world
    /// state for later assertions. Always records both code and message
    /// together so they can't disagree.
    pub fn record_error(&mut self, e: ASCOMError) {
        self.last_error_code = Some(e.code.raw());
        self.last_error = Some(e.message.to_string());
    }
}
