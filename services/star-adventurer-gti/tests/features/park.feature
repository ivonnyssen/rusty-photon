Feature: Park and unpark
  Park stops tracking, slews both axes to encoder position 0 (the mount's
  natural power-up state), and sets AtPark when both axes report stopped.
  Tracking remains disabled after Park (per ASCOM). Unpark clears AtPark
  but does not auto-enable tracking. SetPark is not supported in MVP.

  Scenario: AtPark is false on first connect
    Given a running star-adventurer service
    When I connect the device
    Then AtPark should be false

  Scenario: Park stops tracking before slewing home
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I park the mount
    Then the mount should have received command :K1 before any :S1
    And Tracking should be false

  Scenario: Park targets encoder zero on both axes
    Given a running star-adventurer service
    When I connect the device
    And I park the mount
    Then the mount should have received a :S1 command targeting encoder 0
    And the mount should have received a :S2 command targeting encoder 0

  Scenario: Park is idempotent
    Given a running star-adventurer service
    When I connect the device
    And I park the mount
    And the mount reports both axes stopped at encoder 0
    And I park the mount
    Then the mount should not have received a second :S1 command

  Scenario: AtPark becomes true once both axes settle at encoder 0
    Given a running star-adventurer service
    When I connect the device
    And I park the mount
    And the mount reports both axes stopped at encoder 0
    Then AtPark should eventually be true within 5 seconds

  Scenario: Unpark clears AtPark
    Given a running star-adventurer service
    And the device is parked
    When I unpark the mount
    Then AtPark should be false

  Scenario: Unpark does not auto-enable tracking
    Given a running star-adventurer service
    And the device is parked
    When I unpark the mount
    Then Tracking should be false

  Scenario: SetPark is not implemented in MVP
    Given a running star-adventurer service
    When I connect the device
    And I try to set the park position
    Then the operation should fail with not-implemented

  Scenario: Park fails while disconnected
    Given a running star-adventurer service
    When I try to park the mount
    Then the operation should fail with not-connected
