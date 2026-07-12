Feature: Configuration actions
  The driver exposes its own configuration over HTTP as the vendor ASCOM
  actions `config.get`, `config.apply`, and `config.schema`, mirroring the
  cross-driver protocol in `docs/services/config-actions.md`. `config.get`
  returns the effective configuration (secrets redacted) plus the
  CLI-override-pinned field paths. `config.apply` parses and validates a full
  configuration blob: invalid input returns `status:"invalid"` with field-level
  errors and leaves the file unchanged, while a valid change is persisted
  atomically, classified, and applied through an in-process reload
  (`status:"applying"`). `config.schema` returns a JSON Schema describing the
  configuration's shape plus the editability tiers the web UI renders the form
  from. The actions work whether or not the device is connected.

  Scenario: The config actions are advertised
    Given a running focuser service
    When the supported actions are queried
    Then the supported actions should include config.get, config.apply, and config.schema

  Scenario: The configuration schema is served with its editability tiers
    Given a running focuser service
    When config.schema is called
    Then the schema should describe the serial, server, and focuser sections
    And the schema should mark focuser.unique_id as a locked field
    And the schema should mark server.port as a read-only field

  Scenario: Read the current configuration while disconnected
    Given a running focuser service configured with serial.port /dev/ttyUSB0
    When config.get is called
    Then the config should report serial.port as /dev/ttyUSB0
    And the config should report no overrides

  Scenario: A valid change is persisted and reloaded
    Given a running focuser service
    When config.apply sets max_step to 32000
    Then the apply status should be applying
    And the reload list should include focuser.max_step

  Scenario: A reloaded server rebinds its port and serves the new configuration
    Given a running focuser service
    When config.apply pins the bound port and sets max_step to 32000
    Then the reloaded service serves max_step 32000

  Scenario: An invalid configuration is rejected and not persisted
    Given a running focuser service
    When config.apply is called with an invalid baud_rate
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given a running focuser service
    When the action "config.frobnicate" is called
    Then the call should fail with an action-not-implemented error
