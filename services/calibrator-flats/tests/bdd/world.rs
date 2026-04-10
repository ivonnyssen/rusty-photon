#![allow(dead_code)]
//! BDD test world for calibrator-flats service

use cucumber::World;

#[derive(Debug, Default, World)]
pub struct CalibratorFlatsWorld {
    // Placeholder — will be populated as BDD infrastructure is built out.
    // Full integration tests require rp + OmniSim + calibrator-flats running together.
    pub placeholder: bool,
}
