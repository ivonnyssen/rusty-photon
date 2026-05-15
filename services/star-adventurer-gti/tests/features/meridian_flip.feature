@wip
Feature: Meridian-flip support
  Phase 6 introduces driver-planned meridian flips controlled by
  MountConfig.flip_policy. With enabled = false (the shipped default)
  every flip code path is inert: CanSetPierSide reports false,
  SetSideOfPier returns NOT_IMPLEMENTED, DestinationSideOfPier always
  returns the current side, and the slew planner uses the pre-flip
  coordinate pipeline. With enabled = true, CanSetPierSide reports
  true and SetSideOfPier(side) triggers a through-wrap flip slew that
  keeps the OTA on its current celestial target while landing on the
  requested pier side. The slew planner picks the side via the
  decision tree shared with DestinationSideOfPier: stay on the
  current side when its safety envelope covers the target HA, flip
  to the opposite side otherwise.

  Through-wrap routing is observable on the wire: a flip slew from
  the Northern-Hemisphere pre-flip side (pierWest at mech_HA ≈ 0)
  toward the post-flip side issues :G1 with the CCW bit set (mode
  byte 01 = Goto+Fast+CCW), routing the RA encoder through the
  negative-mech_HA half (counterweight-below-horizon arc) and the
  encoder wrap at -12 h to the mirror band on the post-flip side.

  Scenario: CanSetPierSide reports false by default
    Given a running star-adventurer service
    When I connect the device
    Then CanSetPierSide should be false

  Scenario: CanSetPierSide reports true when flip_policy is enabled
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    Then CanSetPierSide should be true

  Scenario: SetSideOfPier returns not-implemented when flip_policy is disabled
    Given a running star-adventurer service
    When I connect the device
    And I try to set SideOfPier to East
    Then the operation should fail with not-implemented

  Scenario: SetSideOfPier with Unknown returns invalid-value when flip_policy is enabled
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I try to set SideOfPier to Unknown
    Then the operation should fail with invalid-value

  Scenario: SetSideOfPier refuses when not connected
    Given a star-adventurer service configured with flip_policy enabled
    When I try to set SideOfPier to East
    Then the operation should fail with not-connected

  Scenario: SetSideOfPier refuses while parked
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I park the mount
    And I try to set SideOfPier to East
    Then the operation should fail with invalid-while-parked

  Scenario: SetSideOfPier to the current side succeeds without changing pier side
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I set SideOfPier to West
    Then SideOfPier should be West

  Scenario: SetSideOfPier(East) from pierWest issues a CCW Goto on the RA axis
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I set SideOfPier to East
    Then the mount should have received command :G101

  Scenario: SetSideOfPier(East) marks Slewing while the flip is in progress
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I set SideOfPier to East
    Then Slewing should be true

  Scenario: AbortSlew during a SetSideOfPier flip halts both axes
    Given a star-adventurer service configured with flip_policy enabled
    When I connect the device
    And I set SideOfPier to East
    And I abort the slew
    Then the mount should have received command :L1
    And the mount should have received command :L2

  Scenario: DestinationSideOfPier returns the current side when flip_policy is disabled
    Given a star-adventurer service configured with site latitude 45.0 degrees
    When I connect the device
    And I read DestinationSideOfPier for RA 6.0 hours and Dec 30.0 degrees
    Then DestinationSideOfPier should be West

  Scenario: DestinationSideOfPier returns the current side when target is reachable from it
    Given a star-adventurer service configured with flip_policy enabled and site latitude 45.0 degrees
    When I connect the device
    And I read DestinationSideOfPier for RA 6.0 hours and Dec 30.0 degrees
    Then DestinationSideOfPier should be West
