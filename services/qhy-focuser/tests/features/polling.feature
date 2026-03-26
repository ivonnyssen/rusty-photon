@serial
Feature: Background polling
  The focuser's background polling updates position and temperature,
  observable via the ASCOM Alpaca HTTP API.

  Scenario: Position updates after move completes via polling
    Given a focuser service with fast polling
    When I connect the device
    And I move the focuser to position 500
    And I wait for polling to update
    Then the position should be 500
    And the focuser should not be moving
