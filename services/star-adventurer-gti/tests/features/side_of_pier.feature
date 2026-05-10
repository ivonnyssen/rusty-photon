Feature: Side of pier
  SideOfPier is derived from the RA-axis encoder position (converted to
  mechanical hour angle) and the configured site latitude. In the
  northern hemisphere mechanical HA in [-6, +6) maps to East (OTA on the
  east side of the pier); the rest is West. In the southern hemisphere
  the convention inverts. DestinationSideOfPier is not implemented in
  MVP.

  Scenario Outline: Northern-hemisphere SideOfPier per mechanical HA
    Given a star-adventurer service configured with site latitude 45.0 degrees
    And a mount with CPR 3628800 on both axes
    And the RA-axis encoder reports mechanical hour angle <ha> hours
    And a running star-adventurer service
    When I connect the device
    Then SideOfPier should be <expected>

    Examples:
      | ha    | expected |
      | -6.0  | East     |
      | -5.99 | East     |
      | 0.0   | East     |
      | 5.99  | East     |
      | 6.0   | West     |
      | 11.99 | West     |

  Scenario Outline: Southern-hemisphere SideOfPier inverts the convention
    Given a star-adventurer service configured with site latitude -33.0 degrees
    And a mount with CPR 3628800 on both axes
    And the RA-axis encoder reports mechanical hour angle <ha> hours
    And a running star-adventurer service
    When I connect the device
    Then SideOfPier should be <expected>

    Examples:
      | ha   | expected |
      | -6.0 | West     |
      | 0.0  | West     |
      | 6.0  | East     |

  Scenario: SideOfPier setter is not supported
    Given a running star-adventurer service
    When I connect the device
    And I try to set SideOfPier to East
    Then the operation should fail with not-implemented

  Scenario: DestinationSideOfPier is not implemented in MVP
    Given a running star-adventurer service
    When I connect the device
    And I try to read DestinationSideOfPier for RA 6.0 hours and Dec 30.0 degrees
    Then the operation should fail with not-implemented
