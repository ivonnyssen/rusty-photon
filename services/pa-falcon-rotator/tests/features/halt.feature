Feature: Halt
  Halt writes FH and clears the stored target. After Halt, TargetPosition
  falls back to the current Position — the driver no longer has a pending
  request to report.

  Scenario: Halt issues FH on the wire
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I call Halt
    Then FH should have been sent

  Scenario: TargetPosition after Halt tracks current Position
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 50.00 degrees
    And I call MoveAbsolute with 90.00
    And I call Halt
    Then TargetPosition should track current Position after Halt
