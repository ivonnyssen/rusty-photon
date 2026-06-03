Feature: Configuration actions
  The mount exposes its own configuration over HTTP as the vendor ASCOM actions
  `config.get`, `config.apply`, and `config.schema` — the cross-driver protocol
  in `docs/services/config-actions.md` — *alongside* its existing `ApPark` vendor
  actions. `config.get` returns the effective configuration (secrets redacted).
  `config.apply` validates and persists a full config blob (invalid ->
  `status:"invalid"` with field errors, file unchanged; a valid change ->
  persisted + `status:"applying"` + in-process reload). `config.schema` returns a
  JSON Schema plus editability tiers; the `transport` block is read-only (a
  `usb`/`udp` enum best edited in the config file).

  Scenario: The config actions are advertised alongside the ApPark actions
    Given a running star-adventurer service
    When the supported actions are queried
    Then the supported actions should include config.get, config.apply, and config.schema
    And the supported actions should still include SetPreferredApPark

  Scenario: The configuration schema is served with its editability tiers
    Given a running star-adventurer service
    When config.schema is called
    Then the schema should describe the transport, server, and mount sections
    And the schema should mark mount.unique_id as a locked field
    And the schema should mark transport.kind as a read-only field

  Scenario: Read the current configuration
    Given a running star-adventurer service
    When config.get is called
    Then the config should report no overrides

  Scenario: A valid change is persisted and reloaded
    Given a running star-adventurer service
    When config.apply pins the bound port and sets the mount description to "Renamed Mount"
    Then the apply status should be applying
    And the reloaded service serves mount description "Renamed Mount"

  Scenario: An invalid configuration is rejected
    Given a running star-adventurer service
    When config.apply is called with an empty mount unique_id
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given a running star-adventurer service
    When the action "config.frobnicate" is called
    Then the call should fail with an action-not-implemented error
