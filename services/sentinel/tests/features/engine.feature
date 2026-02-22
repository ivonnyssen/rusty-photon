Feature: Engine lifecycle and polling
  The sentinel engine orchestrates monitor connections, disconnections,
  and polling loops. It must be resilient to individual monitor failures.

  Scenario: Engine connects multiple monitors
    Given monitors "Sky" and "Roof" that connect successfully
    When the engine connects all monitors
    Then no errors should occur

  Scenario: Engine continues when a monitor fails to connect
    Given monitor "Sky" that fails to connect
    And monitor "Roof" that connects successfully
    When the engine connects all monitors
    Then no errors should occur

  Scenario: Engine disconnects multiple monitors
    Given monitors "Sky" and "Roof" that disconnect successfully
    When the engine disconnects all monitors
    Then no errors should occur

  Scenario: Engine continues when a monitor fails to disconnect
    Given monitor "Sky" that fails to disconnect
    And monitor "Roof" that disconnects successfully
    When the engine disconnects all monitors
    Then no errors should occur

  Scenario: Engine polls monitor and updates shared state
    Given a monitor "Sky" that reports "Safe"
    And the engine is configured with that monitor
    When the engine runs and is cancelled after a short delay
    Then monitor "Sky" should have been polled
    And the shared state should show "Safe" for "Sky"
