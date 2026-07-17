@wip
Feature: Credential commands
  doctor auth hash-password hashes one password for hand-written configs
  (the third-party-driver escape hatch). doctor auth rotate mints a fresh
  observatory credential, overwrites the canonical pki/credential copy,
  and re-runs the distribution: the Argon2id hash into every installed
  service's server.auth, the plaintext into every client auth block —
  including blocks an earlier credential already occupied, which is what
  distinguishes rotate from the never-overwriting --fix pass.

  Scenario: hash-password reads stdin and prints an Argon2id hash
    When I run doctor auth hash-password with "correct horse battery staple" on stdin
    Then doctor exits with code 0
    And stdout starts with "$argon2id$"

  Scenario: rotate mints a new credential and re-aligns every copy
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And a config file "sentinel.json" containing:
      """
      { "server": { "port": 11114 } }
      """
    And doctor has already run with --fix
    When I run doctor auth rotate
    Then the pki file "credential" has changed
    And the auth hash at "/server/auth/password_hash" in "ppba-driver.json" verifies against the credential file
    And the sentinel client auth password verifies against the auth hash in "ppba-driver.json"

  Scenario: rotate repairs a mismatched client credential
    Given a config file "ppba-driver.json" whose auth hash is of the password "right-password"
    And a config file "sentinel.json" whose client auth block carries the password "wrong-password"
    When I run doctor auth rotate
    And I run doctor with --json
    Then the report has no checks named "auth.mismatch"
    And the auth hash at "/server/auth/password_hash" in "ppba-driver.json" verifies against the credential file
