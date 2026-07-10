@serial
Feature: Focuser movement
  Absolute positioning, halting, position/is-moving reporting, and live
  temperature for the ZWO EAF (M1-M14). Move is absolute-only (Absolute
  is always true) — there is no relative-move prior art in this codebase
  to diverge from. TempComp/TempCompAvailable/SetTempComp stay stubbed,
  matching qhy-focuser/pa-scops-oag; Temperature returns the live
  EAFGetTemp reading, unlike pa-scops-oag which has no sensor.

  Background:
    Given the zwo-focuser service running with the simulation backend

  Scenario: Absolute is always true
    Then focuser device 0 reports Absolute as true

  Scenario: MaxStep and MaxIncrement report the device's fixed maximum
    Then focuser device 0 reports MaxStep as 7000
    And focuser device 0 reports MaxIncrement as 7000

  Scenario: Moving to a position within range starts the move
    Given focuser device 0 is connected
    When I move focuser device 0 to position 3000
    Then focuser device 0 reports IsMoving as true

  Scenario: IsMoving settles to false once the move completes
    Given focuser device 0 is connected
    When I move focuser device 0 to position 3000
    Then focuser device 0 reports IsMoving as true
    And focuser device 0 reports IsMoving as false

  Scenario: Position reflects the target once the move completes
    Given focuser device 0 is connected
    When I move focuser device 0 to position 3000
    Then focuser device 0 reports Position as 3000

  Scenario: Moving out of range is rejected
    Given focuser device 0 is connected
    When I try to move focuser device 0 to position 8000
    Then the move is rejected with ASCOM INVALID_VALUE

  Scenario: Moving to a negative position is rejected
    Given focuser device 0 is connected
    When I try to move focuser device 0 to position -1
    Then the move is rejected with ASCOM INVALID_VALUE

  Scenario: A second move while already moving is rejected
    Given focuser device 0 is connected
    When I move focuser device 0 to position 1000
    And I try to move focuser device 0 to position 2000
    Then the move is rejected with ASCOM INVALID_OPERATION

  Scenario: Halt stops an in-progress move
    Given focuser device 0 is connected
    When I move focuser device 0 to position 5000
    And I halt focuser device 0
    Then focuser device 0 reports IsMoving as false

  Scenario: Halt on an idle focuser succeeds as a no-op
    Given focuser device 0 is connected
    When I halt focuser device 0
    Then focuser device 0 reports IsMoving as false

  Scenario: Temperature reports the live sensor reading
    Given focuser device 0 is connected
    Then focuser device 0 reports Temperature as 20

  Scenario: StepSize is not implemented
    Given focuser device 0 is connected
    When I query step size on focuser device 0
    Then the move is rejected with ASCOM NOT_IMPLEMENTED

  Scenario: TempComp and TempCompAvailable report false
    Then focuser device 0 reports TempComp as false
    And focuser device 0 reports TempCompAvailable as false

  Scenario: SetTempComp is rejected as not implemented
    When I try to set temp comp to true on focuser device 0
    Then the move is rejected with ASCOM NOT_IMPLEMENTED

  Scenario: Move while disconnected is rejected
    When I try to move focuser device 0 to position 100
    Then the move is rejected with ASCOM NOT_CONNECTED

  Scenario: Position while disconnected is rejected
    When I query position on focuser device 0
    Then the move is rejected with ASCOM NOT_CONNECTED
