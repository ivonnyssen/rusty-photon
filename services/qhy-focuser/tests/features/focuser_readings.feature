Feature: Focuser readings
  The focuser reports position, temperature, and movement state
  from cached values populated during connection handshake.

  Scenario: Position read succeeds when connected
    Given a focuser device with standard mock responses
    When I connect the device
    Then the position should be 10000

  Scenario: Position read fails when disconnected
    Given a focuser device with standard mock responses
    When I try to read the position
    Then the operation should fail with not-connected

  Scenario: Temperature read succeeds when connected
    Given a focuser device with standard mock responses
    When I connect the device
    Then the temperature should be approximately 25.0

  Scenario: Temperature read fails when disconnected
    Given a focuser device with standard mock responses
    When I try to read the temperature
    Then the operation should fail with not-connected

  Scenario: IsMoving reports false when connected and idle
    Given a focuser device with standard mock responses
    When I connect the device
    Then the focuser should not be moving

  Scenario: IsMoving fails when disconnected
    Given a focuser device with standard mock responses
    When I try to read is-moving
    Then the operation should fail with not-connected

  Scenario: Cached state is populated after handshake
    Given a serial manager with standard mock responses
    When I connect the serial manager
    Then the cached position should be 10000
    And the cached outer temperature should be approximately 25.0
    And the cached chip temperature should be approximately 30.0
    And the cached voltage should be approximately 12.5
    And the cached firmware version should be "2.1.0"
    And the cached board version should be "1.0"
    And the cached is-moving should be false

  Scenario: Cached state is empty before connection
    Given a serial manager with no responses
    Then the cached position should be empty
    And the cached outer temperature should be empty
    And the cached is-moving should be false
