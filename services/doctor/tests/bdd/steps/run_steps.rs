//! Steps that run the doctor binary.

use cucumber::when;

use crate::world::DoctorWorld;

#[when("I run doctor with --json")]
async fn run_json(world: &mut DoctorWorld) {
    world.run_doctor(true).await;
}

#[when("I run doctor without --json")]
async fn run_text(world: &mut DoctorWorld) {
    world.run_doctor(false).await;
}

#[when("I run doctor with --fix and --json")]
async fn run_fix_json(world: &mut DoctorWorld) {
    world.run_doctor_args(true, true).await;
}

#[when("I run doctor pointed at a config directory that does not exist")]
async fn run_against_missing_dir(world: &mut DoctorWorld) {
    world.config_dir_override = Some(world.temp.path().join("no-such-dir"));
    world.run_doctor(true).await;
}
