@wip
Feature: TLS and credential provisioning under --fix
  The --fix provisioning pass makes an install TLS-on and auth-on: it
  creates a self-signed CA and per-service certificates under the config
  root's pki directory, mints one observatory credential (username
  "observatory", plaintext kept as the canonical 0600 copy at
  pki/credential), writes the Argon2id hash into each installed service's
  server.auth and the plaintext into client auth blocks, and points
  server.tls at the issued material. Absent tls/auth still means plain
  HTTP until --fix runs; present blocks are operator intent and are never
  overwritten.

  Scenario: --fix creates the CA, a service certificate pair, and the credential
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor with --fix and --json
    Then the pki file "ca.pem" exists
    And the pki file "ca-key.pem" exists
    And the pki file "ppba-driver.pem" exists
    And the pki file "ppba-driver-key.pem" exists
    And the pki file "credential" exists
    And the report records an applied "generate-ca" provisioning action
    And the report records an applied "generate-cert" provisioning action for service "ppba-driver"
    And the report records an applied "mint-credential" provisioning action

  Scenario: --fix writes server.tls and server.auth pointing at the issued material
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor with --fix and --json
    Then the config file "ppba-driver.json" points its tls block at the pki pair for "ppba-driver"
    And the config file "ppba-driver.json" has the string "observatory" at "/server/auth/username"
    And the auth hash at "/server/auth/password_hash" in "ppba-driver.json" verifies against the credential file

  Scenario: The diagnosis warns about an installed service serving plain HTTP
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "tls.absent" for service "ppba-driver"
    And the report contains a "warn" check named "auth.absent" for service "ppba-driver"
    And no pki directory exists

  Scenario: A hand-set auth block survives --fix untouched
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112, "auth": { "username": "custom", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$YWJjZGVmZ2g$aGFuZHNldA" } } }
      """
    When I run doctor with --fix and --json
    Then the config file "ppba-driver.json" has the string "custom" at "/server/auth/username"
    And the config file "ppba-driver.json" points its tls block at the pki pair for "ppba-driver"

  Scenario: --fix distributes the plaintext to sentinel's client auth block
    Given a config file "sentinel.json" containing:
      """
      { "server": { "port": 11114 } }
      """
    And a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor with --fix and --json
    Then the sentinel client auth block carries username "observatory"
    And the sentinel client auth password verifies against the auth hash in "ppba-driver.json"
    And the sentinel client CA path points at the pki file "ca.pem"

  Scenario: A mismatched client credential is reported, not rewritten
    Given a config file "ppba-driver.json" whose auth hash is of the password "right-password"
    And a config file "sentinel.json" whose client auth block carries the password "wrong-password"
    When I run doctor with --json
    Then the report contains a "warn" check named "auth.mismatch"
    And that check's suggestion mentions "doctor auth rotate"

  Scenario: A second --fix run applies nothing and keeps the credential
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor has already run with --fix
    When I run doctor with --fix and --json
    Then the report records no applied fixes
    And the pki file "credential" is unchanged
    And the pki file "ca.pem" is unchanged

  Scenario: A service appearing after the first --fix is wired with the same credential
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor has already run with --fix
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11119 } }
      """
    When I run doctor with --fix and --json
    Then the pki file "credential" is unchanged
    And the auth hash at "/server/auth/password_hash" in "dsd-fp2.json" verifies against the credential file
    And the pki file "dsd-fp2.pem" exists

  Scenario: A default run creates no pki tree
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor with --json
    Then no pki directory exists
    And the config file "ppba-driver.json" is unchanged from what was staged
