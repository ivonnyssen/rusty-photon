@serial
Feature: rp hash-password CLI command
  rp can hash a password for use in service auth configuration.

  Scenario: hash-password produces valid Argon2id hash
    When rp hash-password is executed with a test password via stdin
    Then the output should be a valid Argon2id hash string
    And the hash should verify against the original password
