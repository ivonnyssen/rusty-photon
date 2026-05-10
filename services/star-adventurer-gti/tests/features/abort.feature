Feature: Abort slew
  AbortSlew issues :L (instant stop) on both axes, clears the Slewing
  flag, and does NOT auto-restore tracking. AbortSlew while not slewing
  is a no-op (idempotent).

  Scenario: AbortSlew issues :L on both axes
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And I abort the slew
    Then the mount should have received command :L1
    And the mount should have received command :L2

  Scenario: AbortSlew clears Slewing
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And I abort the slew
    Then Slewing should eventually be false within 5 seconds

  Scenario: AbortSlew does not auto-restore tracking
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And I abort the slew
    Then Tracking should be false

  Scenario: AbortSlew while idle is a no-op
    Given a running star-adventurer service
    When I connect the device
    And I abort the slew
    Then the operation should succeed
    And Slewing should be false

  Scenario: AbortSlew fails while disconnected
    Given a running star-adventurer service
    When I try to abort the slew
    Then the operation should fail with not-connected
