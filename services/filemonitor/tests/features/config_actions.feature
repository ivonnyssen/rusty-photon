Feature: Configuration actions
  The driver exposes its own configuration over HTTP as the vendor ASCOM
  actions `config.get`, `config.apply`, and `config.schema`. `config.get`
  returns the effective configuration (secrets redacted) plus the
  CLI-override-pinned field paths (always empty for filemonitor — the binary
  takes no CLI overrides). `config.apply` parses and validates a full
  configuration blob: invalid input returns `status:"invalid"` with
  field-level errors and leaves the file unchanged, while a valid change is
  persisted atomically, classified, and applied through an in-process reload
  (`status:"applying"`). `config.schema` returns a JSON Schema describing the
  configuration's shape plus the editability tiers (identity/locked and hard
  read-only fields) the web UI renders the form from. The actions work
  whether or not the device is connected.

  Scenario: The config actions are advertised
    Given filemonitor is running
    When the supported actions are queried
    Then the supported actions should include config.get and config.apply

  Scenario: The configuration schema is served with its editability tiers
    Given filemonitor is running
    When config.schema is called
    Then the schema should describe the device, file, parsing, and server sections
    And the schema should mark device.unique_id as a locked field
    And the schema should mark server.port as a read-only field

  Scenario: Read the current configuration while disconnected
    Given filemonitor is running
    When config.get is called
    Then the config should report device.unique_id as test-001
    And the config should report no overrides

  Scenario: A valid change is persisted and reloaded
    Given filemonitor is running
    When config.apply sets the polling interval to 30s
    Then the apply status should be applying
    And the reload list should include file.polling_interval

  Scenario: A reloaded server rebinds its port and serves the new configuration
    Given filemonitor is running
    When config.apply pins the bound port and sets the polling interval to 45s
    Then the reloaded service serves polling interval 45s

  Scenario: An invalid configuration is rejected and not persisted
    Given filemonitor is running
    When config.apply is called with an empty unique_id
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An invalid regex pattern is rejected and not persisted
    Given filemonitor is running
    When config.apply is called with an invalid regex pattern
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given filemonitor is running
    When the action "config.frobnicate" is called
    Then the call should fail with an action-not-implemented error
