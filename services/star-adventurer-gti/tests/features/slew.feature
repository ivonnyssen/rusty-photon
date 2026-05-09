@wip
Feature: Asynchronous slewing
  SlewToCoordinatesAsync validates the target, computes target encoder
  positions from RA/Dec + LST + sync offset + side-of-pier choice, then
  issues :G / :S / :J on each axis and returns immediately. Callers poll
  Slewing to detect completion. SlewToTargetAsync uses the most-recent
  TargetRightAscension / TargetDeclination set on the device.

  Scenario: SlewToCoordinatesAsync rejects RA out of range
    Given a running star-adventurer service
    When I connect the device
    And I try to slew asynchronously to RA 24.0 hours and Dec 0.0 degrees
    Then the operation should fail with invalid-value

  Scenario: SlewToCoordinatesAsync rejects RA below zero
    Given a running star-adventurer service
    When I connect the device
    And I try to slew asynchronously to RA -0.1 hours and Dec 0.0 degrees
    Then the operation should fail with invalid-value

  Scenario: SlewToCoordinatesAsync rejects Dec above +90
    Given a running star-adventurer service
    When I connect the device
    And I try to slew asynchronously to RA 0.0 hours and Dec 90.1 degrees
    Then the operation should fail with invalid-value

  Scenario: SlewToCoordinatesAsync rejects Dec below -90
    Given a running star-adventurer service
    When I connect the device
    And I try to slew asynchronously to RA 0.0 hours and Dec -90.1 degrees
    Then the operation should fail with invalid-value

  Scenario: SlewToCoordinatesAsync fails while parked
    Given a running star-adventurer service
    And the device is parked
    When I try to slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    Then the operation should fail with invalid-while-parked

  Scenario: SlewToCoordinatesAsync issues :G :S :J on both axes
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    Then the mount should have received commands matching:
      | pattern  |
      | :G1.*    |
      | :S1.*    |
      | :J1      |
      | :G2.*    |
      | :S2.*    |
      | :J2      |

  Scenario: SlewToCoordinatesAsync remembers the target
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    Then TargetRightAscension should be 6.0 hours within 0.001
    And TargetDeclination should be 30.0 degrees within 0.001

  Scenario: SlewToTargetAsync without a stored target fails
    Given a running star-adventurer service
    When I connect the device
    And I try to slew to the stored target
    Then the operation should fail with invalid-operation

  Scenario: SlewToTargetAsync uses the last set target
    Given a running star-adventurer service
    When I connect the device
    And I set TargetRightAscension to 12.0 hours
    And I set TargetDeclination to 45.0 degrees
    And I slew to the stored target
    Then the slew target on the wire should correspond to RA 12.0 hours and Dec 45.0 degrees

  Scenario: Slewing returns true while a slew is in progress
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    Then Slewing should be true

  Scenario: Slewing returns false after both axes stop
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And the mount reports both axes stopped in goto mode
    Then Slewing should eventually be false within 5 seconds

  Scenario: Tracking resumes after a slew completes
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And the mount reports both axes stopped in goto mode
    Then the mount should eventually receive a tracking-mode :G1 within 5 seconds

  Scenario: Tracking does not resume after a slew if it was off
    Given a running star-adventurer service
    When I connect the device
    And I slew asynchronously to RA 6.0 hours and Dec 30.0 degrees
    And the mount reports both axes stopped in goto mode
    Then the mount should not receive a tracking-mode :G1
