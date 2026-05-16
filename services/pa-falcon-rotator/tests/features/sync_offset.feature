@wip
Feature: Driver-side Sync
  ASCOM Sync(skyAngle) sets the value reported by Position to skyAngle while
  leaving MechanicalPosition unchanged. The Falcon's SD command rewrites the
  device's stored counter, which would change MechanicalPosition on the next
  FA read — so the driver tracks the offset in software and never issues SD.

  Scenario: Sync shifts Position without changing MechanicalPosition
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 142.30 degrees
    And I call Sync with 37.50
    Then no SD command should have been sent
    And MechanicalPosition should be unchanged

  Scenario: After Sync, Position reports the synced sky angle
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 142.30 degrees
    And I call Sync with 37.50
    And I read Position
    Then Position should be 37.50 degrees

  Scenario Outline: Sync rejects non-finite angles with INVALID_VALUE
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I call Sync with <value>
    Then Sync should fail with code 1025

    Examples:
      | value     |
      | NaN       |
      | Infinity  |
      | -Infinity |
