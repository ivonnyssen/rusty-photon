Feature: Movement control
  The focuser can be moved to absolute positions and halted
  via the ASCOM Alpaca HTTP API.

  Scenario: Move to valid position succeeds and sets is-moving
    Given a running focuser service
    When I connect the device
    And I move the focuser to position 20000
    Then the focuser should be moving

  Scenario: Move to negative position rejected with invalid-value
    Given a running focuser service
    When I connect the device
    And I try to move the focuser to position -1
    Then the operation should fail with invalid-value

  Scenario: Move beyond max step rejected with invalid-value
    Given a running focuser service
    When I connect the device
    And I try to move the focuser to position 100000
    Then the operation should fail with invalid-value

  Scenario: Move fails when not connected
    Given a running focuser service
    When I try to move the focuser to position 5000
    Then the operation should fail with not-connected

  Scenario: Halt stops movement
    Given a running focuser service
    When I connect the device
    And I move the focuser to position 50000
    And I halt the focuser
    Then the focuser should not be moving

  Scenario: Halt fails when not connected
    Given a running focuser service
    When I try to halt the focuser
    Then the operation should fail with not-connected

  Scenario: Move completes when position reaches target
    Given a running focuser service
    When I connect the device
    And I move the focuser to position 500
    And I wait for the move to complete
    Then the position should be 500
    And the focuser should not be moving
