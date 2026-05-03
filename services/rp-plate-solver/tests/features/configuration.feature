Feature: Configuration validation at startup

  The wrapper validates its config before binding the HTTP listener. On
  any validation failure it logs a structured error naming the offending
  field and exits non-zero, so Sentinel surfaces the misconfiguration
  rather than masking it with a silent retry.

  Required fields exit on absence — `astap_binary_path` and
  `astap_db_directory` have no implicit defaults. The validation rules
  are listed in `docs/services/rp-plate-solver.md` §"Configuration
  Validation at Startup".

  Scenario: Missing astap_binary_path field rejects the config
    Given a config without astap_binary_path
    When the wrapper starts
    Then the wrapper exits non-zero
    And the wrapper stderr names "astap_binary_path"
    And the wrapper stderr references the README

  Scenario: astap_binary_path pointing at a non-existent file rejects the config
    Given a config with astap_binary_path "/does/not/exist/astap_cli"
    And a valid astap_db_directory
    When the wrapper starts
    Then the wrapper exits non-zero
    And the wrapper stderr names "astap_binary_path"

  Scenario: astap_binary_path pointing at a non-executable file rejects the config
    Given a config with astap_binary_path pointing at a non-executable file
    And a valid astap_db_directory
    When the wrapper starts
    Then the wrapper exits non-zero
    And the wrapper stderr names "not executable"

  Scenario: Missing astap_db_directory field rejects the config
    Given a config without astap_db_directory
    When the wrapper starts
    Then the wrapper exits non-zero
    And the wrapper stderr names "astap_db_directory"

  Scenario: astap_db_directory pointing at a missing path rejects the config
    Given a config with astap_db_directory "/does/not/exist/d05"
    And a valid astap_binary_path
    When the wrapper starts
    Then the wrapper exits non-zero
    And the wrapper stderr names "astap_db_directory"

  Scenario: Valid config with mock_astap as binary path is accepted
    Given a config with mock_astap as the binary path
    And a valid astap_db_directory
    When the wrapper starts
    Then the wrapper prints bound_addr to stdout
    And the wrapper /health returns 200
