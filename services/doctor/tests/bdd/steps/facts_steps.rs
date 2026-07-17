//! Steps that stage the scenario's platform facts.

use std::path::PathBuf;

use cucumber::gherkin::Step;
use cucumber::given;
use doctor::facts::{Platform, UnitFacts};

use crate::world::DoctorWorld;

#[given(expr = "platform facts with an enabled unit {string}")]
fn one_unit(world: &mut DoctorWorld, unit: String) {
    world.add_unit(&unit);
}

#[given("platform facts with enabled units:")]
fn units_table(world: &mut DoctorWorld, step: &Step) {
    let table = step.table().expect("step needs a table");
    for row in table.rows.iter().skip(1) {
        world.add_unit(&row[0]);
    }
}

#[given("platform facts with no rusty-photon units")]
fn no_units(world: &mut DoctorWorld) {
    world.facts.units.clear();
}

#[given(expr = "Windows platform facts with an enabled unit {string}")]
fn windows_unit(world: &mut DoctorWorld, unit: String) {
    world.facts.platform = Platform::Windows;
    world.add_unit(&unit);
}

#[given(expr = "platform facts where enabled unit {string} is gated on a missing file")]
fn unit_gated_missing(world: &mut DoctorWorld, unit: String) {
    let gate = world.temp.path().join("absent-config.json");
    push_gated_unit(world, unit, gate);
}

#[given(expr = "platform facts where enabled unit {string} is gated on config file {string}")]
fn unit_gated_on_config(world: &mut DoctorWorld, unit: String, config: String) {
    let gate = world.config_dir().join(config);
    push_gated_unit(world, unit, gate);
}

fn push_gated_unit(world: &mut DoctorWorld, unit: String, gate: PathBuf) {
    world.facts.units.push(UnitFacts {
        name: unit,
        enabled: true,
        condition_path: Some(gate),
        source_name: None,
    });
}

#[given("the platform facts say no polkit rule grants sentinel restarts")]
fn polkit_absent(world: &mut DoctorWorld) {
    world.facts.polkit_grants_sentinel_restart = Some(false);
}

#[given("the platform facts say a polkit rule grants sentinel restarts")]
fn polkit_present(world: &mut DoctorWorld) {
    world.facts.polkit_grants_sentinel_restart = Some(true);
}
