@serial
Feature: Background polling
  The serial manager polls the focuser for position and temperature updates.

  Scenario: Polling updates position and temperature after connection
    Given a serial manager with fast polling and updated values
    When I connect the serial manager
    And I wait for polling to update
    Then the cached position should be 2000
    And the cached outer temperature should be approximately 28.0
    And the cached chip temperature should be approximately 33.0
    And the cached voltage should be approximately 13.0
