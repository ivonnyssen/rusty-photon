Feature: Configuration loading and validation
  The filemonitor loads its configuration from a JSON file.
  Invalid or missing configuration should fail with clear errors.

  Scenario: Load and validate a configuration file
    Given a configuration file at "tests/config.json"
    When I load the configuration
    Then the device name should be "File Safety Monitor"
    And the unique ID should be "filemonitor-001"
    And the polling interval should be 60 seconds
    And there should be 3 parsing rules
    And the server port should be 11111
    And rule 1 should have pattern "OPEN" and be safe
    And rule 2 should have pattern "CLOSED" and be unsafe
    And case sensitivity should be disabled

  Scenario: Binary starts with valid configuration and serves metadata
    Given filemonitor is running with configuration "tests/config.json"
    Then the name should be "File Safety Monitor"
    And the description should be "ASCOM Alpaca SafetyMonitor that monitors file content"
    And the driver info should be "ASCOM Alpaca SafetyMonitor that monitors file content"
    And the driver version should be "0.1.0"

  Scenario Outline: Reject invalid configuration sources
    Given a configuration file at "<path>"
    When I try to start filemonitor with this configuration
    Then the binary should fail to start

    Examples:
      | path                      |
      | tests/invalid_config.json |
      | nonexistent_config.json   |
