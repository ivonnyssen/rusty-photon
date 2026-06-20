@wip @serial
Feature: Filter wheel
  When filterwheel.enabled is set, zwo-camera registers each discovered EFW
  as an ASCOM FilterWheel device alongside the cameras. Names lists the
  configured filter_names or generated Filter0..FilterN when none are given
  (FW1). Position returns the current slot, or the ASCOM moving sentinel
  while the target slot differs from the actual slot -- EFWGetPosition writes
  -1 into its out-parameter while moving, distinct from the EFW_ERROR_MOVING
  enum, and that -1 maps onto the ASCOM moving sentinel. set_position
  validates that the index is less than the filter count and rejects an
  out-of-range index with INVALID_VALUE (FW2). FocusOffsets returns zero for
  every filter in v0 because the EFW SDK exposes no per-slot offsets (FW3).
  The simulated EFW has 7 positions.

  Background:
    Given the zwo-camera service running with the simulation backend and the filter wheel enabled
    And filterwheel device 0 is connected

  Scenario: The filter wheel exposes seven generated filter names
    Then filterwheel device 0 reports 7 filter names
    And filterwheel device 0 reports the generated names Filter0 through Filter6

  Scenario: Moving to a valid slot updates the reported position
    When I set filterwheel device 0 to position 3
    And the filter wheel move on device 0 completes
    Then filterwheel device 0 reports Position as 3

  Scenario: Position reports the moving sentinel while a move is in progress
    When I set filterwheel device 0 to position 5
    Then filterwheel device 0 reports Position as the moving sentinel

  Scenario Outline: An out-of-range slot is rejected
    When I try to set filterwheel device 0 to position <slot>
    Then the set is rejected with ASCOM INVALID_VALUE

    Examples:
      | slot |
      | 7    |
      | 99   |

  Scenario: Focus offsets are zero for every filter
    Then filterwheel device 0 reports FocusOffsets of 7 zeros

  Scenario: Custom filter names from config are reported
    Given the zwo-camera service running with the filter wheel enabled and filter names L, R, G, B, Ha, OIII, SII
    And filterwheel device 0 is connected
    Then filterwheel device 0 reports the filter names L, R, G, B, Ha, OIII, SII
