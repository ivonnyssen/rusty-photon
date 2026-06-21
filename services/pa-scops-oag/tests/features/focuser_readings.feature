Feature: Focuser readings
  The focuser reports position and movement state via the ASCOM Alpaca HTTP API.
  The Scops OAG has no temperature sensor, so temperature is not implemented.

  Scenario: Position read succeeds when connected
    Given a running focuser service
    When I connect the device
    Then the position should be 0

  Scenario: Position read fails when disconnected
    Given a running focuser service
    When I try to read the position
    Then the operation should fail with not-connected

  Scenario: Temperature is not implemented because there is no sensor
    Given a running focuser service
    When I connect the device
    And I try to read the temperature
    Then the operation should fail with not-implemented

  Scenario: IsMoving reports false when connected and idle
    Given a running focuser service
    When I connect the device
    Then the focuser should not be moving

  Scenario: IsMoving fails when disconnected
    Given a running focuser service
    When I try to read is-moving
    Then the operation should fail with not-connected
