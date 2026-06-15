@wip
Feature: Configuration actions
  The driver exposes its own configuration over HTTP as the vendor ASCOM
  actions `config.get`, `config.apply`, and `config.schema`, mirroring the
  cross-driver protocol in `docs/services/config-actions.md`. `config.get`
  returns the effective configuration plus the CLI-override-pinned field
  paths. `config.schema` returns a JSON Schema plus the editability tiers:
  server.port and filterwheel.enabled are read-only, and there is NO locked
  identity field because UniqueIDs are derived from the camera/EFW SDK serial
  rather than minted into config. `config.apply` parses and validates a full
  configuration blob: invalid input returns `status:"invalid"` with
  field-level errors and leaves the file unchanged, while a valid change to
  the per-serial `devices` map is persisted atomically and applied through an
  in-process reload (`status:"applying"`). The actions work whether or not a
  device is connected.

  Background:
    Given a running zwo-camera service with the simulation backend

  Scenario: The config actions are advertised
    When the supported actions are queried on camera device 0
    Then the supported actions should include config.get, config.apply, and config.schema

  Scenario: The configuration schema is served with its editability tiers
    When config.schema is called
    Then the schema should describe the devices, filterwheel, and server sections
    And the schema should mark server.port as a read-only field
    And the schema should mark filterwheel.enabled as a read-only field
    And the schema should report no locked identity fields

  Scenario: Read the current configuration
    When config.get is called
    Then the config should report an empty devices map
    And the config should report no CLI-pinned override paths

  Scenario: A per-serial device override is persisted and reloaded
    When config.apply sets the devices override "ASI2600MM-SIM" name to "Main Imaging"
    Then the apply status should be applying
    And the reload list should include devices

  Scenario: An empty filter name is rejected and not persisted
    When config.apply sets a filter_names entry to an empty string
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    When the action "config.frobnicate" is called on camera device 0
    Then the call should fail with an action-not-implemented error
