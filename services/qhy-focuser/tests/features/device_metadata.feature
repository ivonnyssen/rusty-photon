Feature: Device metadata
  The focuser device reports its identity and static properties
  via the ASCOM Alpaca HTTP API.

  Scenario: Device reports configured name
    Given a focuser service configured with name "Test Focuser"
    Then the device name should be "Test Focuser"

  Scenario: Device reports configured unique ID
    Given a focuser service configured with unique ID "custom-id-123"
    Then the device unique ID should be "custom-id-123"

  Scenario: Device reports configured description
    Given a focuser service configured with description "Custom description"
    When I connect the device
    Then the device description should be "Custom description"

  Scenario: Device reports driver info containing QHY and Focuser
    Given a running focuser service
    When I connect the device
    Then the driver info should contain "QHY"
    And the driver info should contain "Focuser"

  Scenario: Device reports a non-empty driver version
    Given a running focuser service
    When I connect the device
    Then the driver version should not be empty

  Scenario: Focuser is always absolute
    Given a running focuser service
    When I connect the device
    Then the focuser should be absolute

  Scenario: Max step matches configuration
    Given a focuser service configured with max step 100000
    When I connect the device
    Then the max step should be 100000

  Scenario: Max increment matches configuration
    Given a running focuser service
    When I connect the device
    Then the max increment should be 64000

  Scenario: Temperature compensation is not available
    Given a running focuser service
    When I connect the device
    Then temperature compensation should not be available

  Scenario: Temperature compensation is always off
    Given a running focuser service
    When I connect the device
    Then temperature compensation should be off

  Scenario: Setting temperature compensation fails with not-implemented
    Given a running focuser service
    When I connect the device
    And I try to enable temperature compensation
    Then the operation should fail with not-implemented

  Scenario: Step size is not implemented
    Given a running focuser service
    When I connect the device
    And I try to read step size
    Then the operation should fail with not-implemented

  Scenario: Interface version is reported
    Given a running focuser service
    When I connect the device
    Then the device interface ID should be 4
