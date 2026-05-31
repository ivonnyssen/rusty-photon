Feature: Configuration actions
  The driver registers two ASCOM devices (rotator + status switch) backed by one
  config file, and BOTH expose the vendor actions `config.get`, `config.apply`,
  and `config.schema` — the cross-driver protocol in
  `docs/services/config-actions.md`. An apply on either device operates on the
  same full driver config and fires the same in-process reload. `config.get`
  returns the effective configuration (secrets redacted) plus CLI-override-pinned
  paths; `config.apply` validates and persists a full config blob (invalid ->
  `status:"invalid"` with field errors, file unchanged; a valid change ->
  persisted + `status:"applying"` + reload); `config.schema` returns a JSON
  Schema plus editability tiers covering both devices' identity fields.

  Scenario: Both devices advertise the config actions
    Given a running pa-falcon-rotator service
    When the supported actions are queried on the rotator device
    Then the queried supported actions should include config.get, config.apply, and config.schema
    When the supported actions are queried on the switch device
    Then the queried supported actions should include config.get, config.apply, and config.schema

  Scenario: The configuration schema is served with both devices' editability tiers
    Given a running pa-falcon-rotator service
    When config.schema is called on the rotator device
    Then the schema should describe the serial, server, rotator, and switch sections
    And the schema should mark rotator.unique_id and switch.unique_id as locked fields

  Scenario: Read the current configuration
    Given a running pa-falcon-rotator service
    When config.get is called on the rotator device
    Then the config should report serial.port as /dev/ttyUSB0
    And the config should report no overrides

  Scenario: A valid change is persisted and fires the reload
    Given a running pa-falcon-rotator service
    When config.apply sets the rotator name to "Renamed Rotator"
    Then the apply status should be applying
    And the reload signal should fire
    And the persisted config should report the rotator name as "Renamed Rotator"

  Scenario: An invalid configuration is rejected and not persisted
    Given a running pa-falcon-rotator service
    When config.apply is called with an empty serial port
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given a running pa-falcon-rotator service
    When the action "config.frobnicate" is called on the rotator device
    Then the call should fail with an action-not-implemented error
