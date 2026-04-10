#![allow(dead_code)]
//! BDD test world for panel-flat service

use cucumber::World;

#[derive(Debug, Default, World)]
pub struct PanelFlatWorld {
    // Placeholder — will be populated as BDD infrastructure is built out.
    // Full integration tests require rp + OmniSim + panel-flat running together.
    pub placeholder: bool,
}
