@wip
Feature: Sidereal tracking
  Setting Tracking = true issues :G (tracking mode) + :I (sidereal step
  period) + :J on the RA axis. Setting Tracking = false issues :K on the
  RA axis to decelerate to a stop. The Dec axis is never touched by
  tracking. TrackingRate is read-only and always reports Sidereal in MVP.

  Scenario: Tracking is false on first connect
    Given a running star-adventurer service
    When I connect the device
    Then Tracking should be false

  Scenario: Setting Tracking on issues tracking-mode commands on the RA axis
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    Then Tracking should be true
    And the mount should have received commands matching:
      | pattern |
      | :G1.*   |
      | :I1.*   |
      | :J1     |

  Scenario: Tracking off issues :K1
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I disable tracking
    Then Tracking should be false
    And the mount should have received command :K1

  Scenario: Setting tracking does not touch the Dec axis
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    Then the Dec axis should have received no commands

  Scenario: TrackingRate read returns Sidereal
    Given a running star-adventurer service
    When I connect the device
    Then TrackingRate should be Sidereal

  Scenario: Setting TrackingRate to non-sidereal fails
    Given a running star-adventurer service
    When I connect the device
    And I try to set TrackingRate to Lunar
    Then the operation should fail with invalid-value

  Scenario: Tracking fails while disconnected
    Given a running star-adventurer service
    When I try to enable tracking
    Then the operation should fail with not-connected
