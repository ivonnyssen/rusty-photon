Feature: Filter wheel control
  The filter wheel reports its position and supports position changes.
  While moving, position returns None. Filter names can come from
  configuration or default to "Filter0", "Filter1", etc.

  Scenario: Filter wheel position after connect
    Given a connected filter wheel device
    Then filter wheel position should be 0

  Scenario: Filter wheel position can be changed
    Given a connected filter wheel device
    When I set filter wheel position to 3
    Then filter wheel position should be 3

  Scenario: Setting same position is idempotent
    Given a connected filter wheel device
    When I set filter wheel position to 0
    Then filter wheel position should be 0

  Scenario: Invalid position is rejected
    Given a connected filter wheel device
    When I try to set filter wheel position to 99
    Then the operation should fail with an invalid-value error

  Scenario: Default filter names
    Given a connected filter wheel device
    Then filter names should have 7 entries
    And first filter name should be "Filter0"

  Scenario: Custom filter names from config
    Given a filter wheel config with names "L,R,G,B,Ha,OIII,SII"
    And a connected filter wheel device with config
    Then first filter name should be "L"

  Scenario: Focus offsets are all zeros
    Given a connected filter wheel device
    Then focus_offsets should have 7 entries
    And all focus offsets should be 0

  Scenario: Filter wheel properties fail when not connected
    Given a filter wheel device with mock SDK
    When I try to read filter wheel position
    Then the operation should fail with a not-connected error
