@serial
Feature: Configuration actions
  The driver exposes its own configuration over HTTP as the vendor ASCOM
  actions `config.get`, `config.apply`, and `config.schema`, mirroring the
  cross-driver protocol in `docs/services/config-actions.md`. `config.get`
  returns the effective configuration plus the CLI-override-pinned field
  paths. `config.schema` returns a JSON Schema plus the editability tiers:
  server.port is read-only, and there is NO locked identity field because
  UniqueIDs are derived from the camera SDK serial rather than minted into
  config. `config.apply` parses a full
  configuration blob: malformed input (wrong types, bad JSON) is rejected
  with an INVALID_VALUE error and leaves the file unchanged, while a valid
  change to the per-serial `devices` map is persisted atomically and applied
  through an in-process reload (`status:"applying"`). The actions work
  whether or not a device is connected.

  Background:
    Given a running zwo-camera service with the simulation backend

  Scenario: The config actions are advertised
    When the supported actions are queried on camera device 0
    Then the supported actions should include config.get, config.apply, and config.schema

  Scenario: The configuration schema is served with its editability tiers
    When config.schema is called
    Then the schema should describe the devices and server sections
    And the schema should mark server.port as a read-only field
    And the schema should report no locked identity fields

  Scenario: Read the current configuration
    When config.get is called
    Then the config should report an empty devices map
    And the config should report no CLI-pinned override paths

  Scenario: A per-serial device override is persisted and reloaded
    When config.apply sets the devices override "ASI2600MM-SIM" name to "Main Imaging"
    Then the apply status should be applying
    And the reload list should include devices

  Scenario: A malformed device override is rejected and not persisted
    When config.apply sets a devices override name to a number
    Then the call should fail with an invalid-value error
    When config.get is called
    Then the config should report an empty devices map

  Scenario: An unknown action is not implemented
    When the action "config.frobnicate" is called on camera device 0
    Then the call should fail with an action-not-implemented error
