Feature: Configuration actions
  The camera exposes its own configuration over HTTP as the vendor ASCOM actions
  `config.get`, `config.apply`, and `config.schema` — the cross-driver protocol
  in `docs/services/config-actions.md`. `config.get` returns the effective
  configuration (follow-mode credentials redacted). `config.apply` validates and
  persists a full config blob (invalid -> `status:"invalid"` with field errors,
  file unchanged; a valid change -> persisted + `status:"applying"` + in-process
  reload). `config.schema` returns a JSON Schema plus the editability tiers the
  web UI renders the form from.

  Scenario: The config actions are advertised
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And the supported actions are queried
    Then the queried supported actions should include config.get, config.apply, and config.schema

  Scenario: The configuration schema is served with its editability tiers
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And config.schema is called
    Then the schema should describe the device, optics, pointing, survey, and server sections
    And the schema should mark device.unique_id as a locked field

  Scenario: Read the current configuration
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And config.get is called
    Then the config should report the device name as "Test Sky Survey Camera"
    And the config should report no overrides

  Scenario: A valid change is persisted and reloaded
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And config.apply pins the bound port and sets the device description to "Renamed Camera"
    Then the apply status should be applying
    And the reloaded service serves device description "Renamed Camera"

  Scenario: An invalid configuration is rejected
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And config.apply is called with an empty device unique_id
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given a sky-survey-camera with default optics
    And SkyView is reachable
    When I start the service
    And the action "config.frobnicate" is called
    Then the call should fail with an action-not-implemented error
