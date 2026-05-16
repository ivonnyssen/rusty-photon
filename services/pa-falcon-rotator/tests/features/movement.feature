@wip
Feature: Movement
  Move, MoveAbsolute, and MoveMechanical each translate ASCOM angle semantics
  into a single MD:nn.nn command, store the sky-coordinate target, and return
  immediately. IsMoving is the authoritative completion signal — it reads FA
  on every call and never depends on driver-side bookkeeping.

  Scenario: MoveAbsolute subtracts the sync offset before issuing MD
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 0.00 degrees
    And the driver-side sync offset is -104.80 degrees
    And I call MoveAbsolute with 180.00
    Then MD:284.80 should have been sent

  Scenario: MoveAbsolute with zero sync offset passes the angle through verbatim
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 0.00 degrees
    And the driver-side sync offset is 0.00 degrees
    And I call MoveAbsolute with 45.50
    Then MD:45.50 should have been sent

  Scenario: MoveMechanical does NOT subtract the sync offset from the wire value
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 0.00 degrees
    And the driver-side sync offset is -104.80 degrees
    And I call MoveMechanical with 90.00
    Then MD:90.00 should have been sent

  Scenario: Move adds a relative delta to current mechanical and re-normalises
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 350.00 degrees
    And the driver-side sync offset is 0.00 degrees
    And I call Move with 20.00
    Then MD:10.00 should have been sent

  Scenario: IsMoving reads FA on every call
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I read IsMoving
    Then an FA command should have been issued

  Scenario: MoveAbsolute with NaN is rejected with INVALID_VALUE
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I call MoveAbsolute with NaN
    Then the move should fail with code 1025
