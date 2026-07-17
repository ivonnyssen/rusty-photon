//! Steps that stage the scenario's config directory.

use cucumber::gherkin::Step;
use cucumber::given;

use crate::world::DoctorWorld;

fn docstring(step: &Step) -> &str {
    step.docstring().expect("step needs a docstring").trim()
}

#[given("an empty config directory")]
fn empty_config_dir(_world: &mut DoctorWorld) {}

#[given(expr = "a config directory with a valid {string} on port {int}")]
fn valid_config(world: &mut DoctorWorld, name: String, port: u16) {
    world.write_config(&name, &format!(r#"{{ "server": {{ "port": {port} }} }}"#));
}

#[given(expr = "a config directory with {string} containing:")]
#[given(expr = "a config file {string} containing:")]
fn config_containing(world: &mut DoctorWorld, name: String, step: &Step) {
    world.write_config(&name, docstring(step));
}

#[given(expr = "a config directory where {string} is not valid JSON")]
fn invalid_json_config(world: &mut DoctorWorld, name: String) {
    world.write_config(&name, "{ this is not json");
}

#[given(expr = "a config directory containing PEM files {string} and {string}")]
fn pem_files(world: &mut DoctorWorld, cert: String, key: String) {
    for name in [&cert, &key] {
        world.write_config(name, "-----BEGIN STUB PEM-----\n-----END STUB PEM-----\n");
        let path = world.config_dir().join(name);
        world.pem_paths.push(path);
    }
}

#[given(expr = "a config file {string} with a tls block pointing at those PEM files on port {int}")]
fn config_with_tls(world: &mut DoctorWorld, name: String, port: u16) {
    let (cert, key) = staged_pems(world);
    world.write_config(
        &name,
        &format!(
            r#"{{ "server": {{ "port": {port}, "tls": {{ "cert": "{cert}", "key": "{key}" }} }} }}"#
        ),
    );
}

#[given(
    expr = "a config file {string} with tls and auth blocks pointing at those PEM files on port {int}"
)]
fn config_with_tls_and_auth(world: &mut DoctorWorld, name: String, port: u16) {
    let (cert, key) = staged_pems(world);
    world.write_config(
        &name,
        &format!(
            r#"{{ "server": {{ "port": {port},
                 "tls": {{ "cert": "{cert}", "key": "{key}" }},
                 "auth": {{ "username": "observatory", "password_hash": "$argon2id$stub" }} }} }}"#
        ),
    );
}

fn staged_pems(world: &DoctorWorld) -> (String, String) {
    let path = |i: usize| {
        world.pem_paths[i]
            .to_str()
            .expect("utf8 path")
            .replace('\\', "\\\\")
    };
    assert!(
        world.pem_paths.len() >= 2,
        "stage PEM files before referencing them"
    );
    (path(0), path(1))
}

#[given("a config directory with an existing data directory")]
fn existing_data_dir(world: &mut DoctorWorld) {
    let dir = world.temp.path().join("data");
    std::fs::create_dir(&dir).expect("data dir");
    world.data_dir = Some(dir);
}

#[given(
    expr = "a config file {string} with session.data_directory pointing at that data directory on port {int}"
)]
fn config_with_data_dir(world: &mut DoctorWorld, name: String, port: u16) {
    let dir = world
        .data_dir
        .as_ref()
        .expect("stage the data directory first")
        .to_str()
        .expect("utf8 path")
        .replace('\\', "\\\\");
    world.write_config(
        &name,
        &format!(
            r#"{{ "server": {{ "port": {port} }},
                 "session": {{ "data_directory": "{dir}" }} }}"#
        ),
    );
}
