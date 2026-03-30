use tracing::debug;

/// Run the `hash-password` command: hash a plaintext password with Argon2id.
///
/// In interactive mode, prompts for the password twice for confirmation.
/// With `--stdin`, reads a single line from stdin (for scripting and BDD tests).
pub fn run(stdin_mode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let password = if stdin_mode {
        debug!("reading password from stdin");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end().to_string()
    } else {
        let password = rpassword::prompt_password("Enter password: ")?;
        let confirm = rpassword::prompt_password("Confirm password: ")?;
        if password != confirm {
            return Err("passwords do not match".into());
        }
        password
    };

    if password.is_empty() {
        return Err("password must not be empty".into());
    }

    let hash = rp_auth::credentials::hash_password(&password)?;
    println!("{hash}");
    Ok(())
}
