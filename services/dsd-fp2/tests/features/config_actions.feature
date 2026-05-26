Feature: Configuration actions
  The driver exposes its own configuration over HTTP as the vendor ASCOM
  actions `config.get` and `config.apply`. `config.get` returns the effective
  configuration (secrets redacted) plus the CLI-override-pinned field paths.
  `config.apply` parses and validates a full configuration blob: invalid input
  returns `status:"invalid"` with field-level errors and leaves the file
  unchanged, while a valid change is persisted atomically, classified, and
  applied through an in-process reload (`status:"applying"`). The actions work
  whether or not the device is connected.

  Scenario: The config actions are advertised
    Given a running FP2 service
    When the supported actions are queried
    Then the supported actions should include config.get and config.apply

  Scenario: Read the current configuration while disconnected
    Given a running FP2 service
    When config.get is called
    Then the config should report serial.port as /dev/mock
    And the config should report no overrides

  Scenario: A valid change is persisted and reloaded
    Given a running FP2 service
    When config.apply sets max_brightness to 2048
    Then the apply status should be applying
    And the reload list should include cover_calibrator.max_brightness

  Scenario: A reloaded server rebinds its port and serves the new configuration
    Given a running FP2 service
    When config.apply pins the bound port and sets max_brightness to 2048
    Then the reloaded service serves max_brightness 2048

  Scenario: An invalid configuration is rejected and not persisted
    Given a running FP2 service
    When config.apply is called with an invalid baud_rate
    Then the apply status should be invalid
    And the response should contain validation errors

  Scenario: An unknown action is not implemented
    Given a running FP2 service
    When the action "config.frobnicate" is called
    Then the call should fail with an action-not-implemented error
