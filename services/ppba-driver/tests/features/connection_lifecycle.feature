Feature: Connection Lifecycle
  As an ASCOM client
  I want to manage device connections
  So that I can control power outputs and read sensors

  Scenario: Switch device starts disconnected
    Given a switch device with standard mock responses
    Then the switch device should be disconnected

  Scenario: Switch device connects successfully
    Given a switch device with standard mock responses
    When I connect the switch device
    Then the switch device should be connected

  Scenario: Switch device disconnects successfully
    Given a switch device with standard mock responses
    When I connect the switch device
    And I disconnect the switch device
    Then the switch device should be disconnected

  Scenario: Switch device connect is idempotent
    Given a switch device with standard mock responses
    When I connect the switch device
    And I connect the switch device
    Then the switch device should be connected

  Scenario: Switch device disconnect is idempotent
    Given a switch device with standard mock responses
    When I connect the switch device
    And I disconnect the switch device
    And I disconnect the switch device
    Then the switch device should be disconnected

  Scenario: Switch device survives multiple connect-disconnect cycles
    Given a switch device with standard mock responses
    When I cycle the switch device connection 5 times
    Then the switch device should be disconnected

  Scenario: OC device starts disconnected
    Given an OC device with standard mock responses
    Then the OC device should be disconnected

  Scenario: OC device connects successfully
    Given an OC device with standard mock responses
    When I connect the OC device
    Then the OC device should be connected

  Scenario: OC device disconnects successfully
    Given an OC device with standard mock responses
    When I connect the OC device
    And I disconnect the OC device
    Then the OC device should be disconnected

  Scenario: OC device connect is idempotent
    Given an OC device with standard mock responses
    When I connect the OC device
    And I connect the OC device
    Then the OC device should be connected

  Scenario: OC device disconnect is idempotent
    Given an OC device with standard mock responses
    When I connect the OC device
    And I disconnect the OC device
    And I disconnect the OC device
    Then the OC device should be disconnected

  Scenario: Serial manager starts not available
    Given a serial manager with standard mock responses
    Then the serial manager should not be available

  Scenario: Serial manager connects first device
    Given a serial manager with standard mock responses
    When I connect the serial manager
    Then the serial manager should be available

  Scenario: Serial manager increments refcount on second connect
    Given a serial manager with standard mock responses
    When I connect the serial manager
    And I connect the serial manager
    And I disconnect the serial manager
    Then the serial manager should be available

  Scenario: Serial manager disconnects last device and closes port
    Given a serial manager with standard mock responses
    When I connect the serial manager
    And I disconnect the serial manager
    Then the serial manager should not be available

  Scenario: Serial manager full lifecycle
    Given a serial manager with standard mock responses
    When I connect the serial manager
    Then the serial manager should be available
    When I disconnect the serial manager
    Then the serial manager should not be available

  Scenario: Serial manager disconnect underflow protection
    Given a serial manager with no mock responses
    When I disconnect the serial manager
    Then the serial manager should not be available

  Scenario: Serial manager debug representation
    Given a serial manager with no mock responses
    Then the serial manager debug representation should contain "SerialManager"

  Scenario: Connection fails with factory error
    Given a serial manager with a failing factory "port not found"
    When I try to connect the serial manager
    Then the serial manager should not be available
    And the last operation should have failed

  Scenario: Connection fails with bad ping
    Given a serial manager with bad ping response
    When I try to connect the serial manager
    Then the last operation should have failed

  Scenario: Switch device connection fails with failing factory
    Given a switch device with a failing serial port "mock port not found"
    When I try to connect the switch device
    Then the switch device should be disconnected
    And the last error code should be INVALID_OPERATION

  Scenario: Switch device connection fails with bad ping
    Given a switch device with bad ping response
    When I try to connect the switch device
    Then the last error code should be INVALID_OPERATION

  Scenario: OC device connection fails with failing factory
    Given an OC device with a failing serial port "mock port not found"
    When I try to connect the OC device
    Then the last error code should be INVALID_OPERATION

  Scenario: OC device connection fails with bad ping
    Given an OC device with bad ping response
    When I try to connect the OC device
    Then the last error code should be INVALID_OPERATION
