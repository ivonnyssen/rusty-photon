Feature: Sync to coordinates
  SyncToCoordinates writes the supplied RA / Dec to the mount via :E on
  each axis (which sets the encoder position) and updates the in-memory
  sync offset so subsequent RA / Dec reads reflect the new alignment.
  SyncToTarget syncs to the most-recent TargetRightAscension / Declination.

  Scenario: SyncToCoordinates fails while disconnected
    Given a running star-adventurer service
    When I try to sync to RA 6.0 hours and Dec 30.0 degrees
    Then the operation should fail with not-connected

  Scenario: SyncToCoordinates rejects RA out of range
    Given a running star-adventurer service
    When I connect the device
    And I try to sync to RA 24.5 hours and Dec 0.0 degrees
    Then the operation should fail with invalid-value

  Scenario: SyncToCoordinates rejects Dec out of range
    Given a running star-adventurer service
    When I connect the device
    And I try to sync to RA 0.0 hours and Dec 91.0 degrees
    Then the operation should fail with invalid-value

  Scenario: SyncToCoordinates fails while parked
    Given a running star-adventurer service
    And the device is parked
    When I try to sync to RA 6.0 hours and Dec 30.0 degrees
    Then the operation should fail with invalid-while-parked

  Scenario: SyncToCoordinates issues :E on both axes
    Given a running star-adventurer service
    When I connect the device
    And I sync to RA 6.0 hours and Dec 30.0 degrees
    Then the mount should have received commands matching:
      | pattern |
      | :E1.*   |
      | :E2.*   |

  Scenario: After sync, RightAscension reads the synced value
    Given a running star-adventurer service
    When I connect the device
    And I sync to RA 6.0 hours and Dec 30.0 degrees
    Then RightAscension should be 6.0 hours within 0.001
    And Declination should be 30.0 degrees within 0.001

  Scenario: SyncToTarget without a stored target fails
    Given a running star-adventurer service
    When I connect the device
    And I try to sync to the stored target
    Then the operation should fail with invalid-operation

  Scenario: SyncToTarget uses the last set target
    Given a running star-adventurer service
    When I connect the device
    And I set TargetRightAscension to 8.0 hours
    And I set TargetDeclination to 45.0 degrees
    And I sync to the stored target
    Then RightAscension should be 8.0 hours within 0.001
    And Declination should be 45.0 degrees within 0.001
