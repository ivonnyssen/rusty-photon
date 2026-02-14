Feature: Connection lifecycle
  The focuser device can be connected and disconnected.
  The serial manager uses reference-counted connections.

  Scenario: Device starts disconnected
    Given a focuser device with standard mock responses
    Then the device should be disconnected

  Scenario: Device connects successfully
    Given a focuser device with standard mock responses
    When I connect the device
    Then the device should be connected

  Scenario: Device disconnects after connect
    Given a focuser device with standard mock responses
    When I connect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Connecting an already-connected device is idempotent
    Given a focuser device with standard mock responses
    When I connect the device
    And I connect the device
    Then the device should be connected

  Scenario: Connection fails when serial port is unavailable
    Given a focuser device with a failing serial port "port busy"
    When I try to connect the device
    Then connecting should fail with an error containing "port busy"

  Scenario: Second connect increments ref-count, first disconnect keeps connection alive
    Given a serial manager with standard mock responses
    When I connect the serial manager
    And I connect the serial manager
    And I disconnect the serial manager
    Then the serial manager should be available
    When I disconnect the serial manager
    Then the serial manager should not be available

  Scenario: Disconnect at zero ref-count is a no-op
    Given a serial manager with no responses
    When I disconnect the serial manager
    Then the serial manager should not be available
