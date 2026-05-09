@wip
@serial
Feature: Center on target compound tool
  The center_on_target MCP tool drives an iterative
  capture → plate_solve → sync_mount → slew loop until the great-circle
  residual between the solved field-center and the requested (ra, dec)
  sits at or below tolerance_arcsec. ra is decimal hours [0, 24); dec is
  decimal degrees [-90, 90]; the input is converted to degrees once for
  the residual check (the solved values are degrees on the wire). On the
  first iteration the tool issues sync_mount with the solved center
  unconditionally — the first solve is the absolute pointing reference
  and subsequent iterations rely on the mount honouring relative slews
  rather than re-syncing. After sync, if the residual is already inside
  tolerance the loop returns without slewing; otherwise it slews to
  (ra, dec) and continues. Subsequent iterations skip the sync, slew on
  miss, and return on hit. The loop errors with tolerance_not_reached
  after max_attempts and propagates any per-iteration capture /
  plate_solve / sync_mount / slew failure verbatim. center_on_target
  does not write a section on any single exposure document — each
  per-iteration capture's wcs section is written by the embedded
  plate_solve, and the compound result is returned via MCP plus
  centering_started / centering_iteration / centering_complete events.
  Inputs (camera_id, ra, dec, duration, tolerance_arcsec, max_attempts)
  are required; max_attempts is capped at 50 (MAX_ATTEMPTS) before any
  motion. The mount is resolved via the singular mount config — there is
  no telescope_id parameter.

  Scenario: Tool catalog includes center_on_target
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "center_on_target"

  Scenario: Single-iteration happy path returns converged immediately
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "0.7123" dec "41.269"
    And the MCP client calls center_on_target with camera "main-cam" ra 0.7123 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should succeed
    And the center_on_target result should report attempts 1
    And the center_on_target iterations[0] action should be "converged"
    And the center_on_target result should contain "final_ra"
    And the center_on_target result should contain "final_dec"
    And the center_on_target result should contain "final_error_arcsec"

  Scenario: Multi-iteration happy path converges on iteration 2
    Given a running Alpaca simulator
    And a stub plate solver returning these per-call WCS responses:
      | ra_center | dec_center |
      | 9.9000    | 41.0000    |
      | 10.6850   | 41.2690    |
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "10.6848" dec "41.269"
    And the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should succeed
    And the center_on_target result should report attempts 2
    And the center_on_target iterations[0] action should be "sync"
    And the center_on_target iterations[1] action should be "converged"
    And the stub plate solver should have received 2 solve calls

  Scenario: tolerance_not_reached after max_attempts
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "20.0000" dec "-30.0000"
    And the MCP client calls center_on_target with camera "main-cam" ra 20.0000 dec -30.0000 duration "100ms" tolerance_arcsec 1 max_attempts 2
    Then the tool call should return an error
    And the error message should contain "tolerance_not_reached"

  Scenario: Mid-loop plate_solve failure aborts and propagates
    Given a running Alpaca simulator
    And a stub plate solver returning error code "solve_failed" with message "ASTAP exited with code 1"
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "10.6848" dec "41.269"
    And the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should return an error
    And the error message should contain "solve_failed"

  Scenario: Mid-loop slew failure (tracking off) aborts and propagates
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to false
    And an MCP client connected to rp
    When the MCP client calls center_on_target with camera "main-cam" ra 20.0000 dec -30.0000 duration "100ms" tolerance_arcsec 1 max_attempts 5
    Then the tool call should return an error

  Scenario: sync-on-iter-1-only invariant — final mount position reflects last slew, not last solve
    Given a running Alpaca simulator
    And a stub plate solver returning these per-call WCS responses:
      | ra_center | dec_center |
      | 9.9000    | 41.0000    |
      | 10.0000   | 41.1000    |
      | 10.6850   | 41.2690    |
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "10.6848" dec "41.269"
    And the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    And the MCP client calls "get_mount_position"
    Then the tool call should succeed
    And the mount position should be approximately ra 10.6848 dec 41.269

  Scenario: Per-iteration wcs sections persist on every captured document
    Given rp's data_directory is pinned to a fresh tempdir
    And a running Alpaca simulator
    And a stub plate solver returning these per-call WCS responses:
      | ra_center | dec_center |
      | 9.9000    | 41.0000    |
      | 10.6850   | 41.2690    |
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls "sync_mount" with ra "10.6848" dec "41.269"
    And the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then 2 FITS files should exist in the pinned data directory
    And every sidecar JSON in the pinned data directory should contain an "wcs" section

  Scenario: Nonexistent camera returns error
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls center_on_target with camera "nonexistent" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should return an error
    And the error message should contain "camera not found"

  Scenario: Disconnected camera returns error
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with an unreachable camera and a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should return an error
    And the error message should contain "camera not connected"

  Scenario: No mount configured returns error
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should return an error
    And the error message should contain "no mount configured"

  Scenario: Mount not connected returns error
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera on the simulator and an unreachable mount
    And an MCP client connected to rp
    When the MCP client calls center_on_target with camera "main-cam" ra 10.6848 dec 41.269 duration "100ms" tolerance_arcsec 60 max_attempts 5
    Then the tool call should return an error
    And the error message should contain "mount not connected"

  Scenario Outline: Rejects calls missing required parameters
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls center_on_target omitting "<missing_param>"
    Then the tool call should return an error
    And the error message should contain "<missing_param>"

    Examples:
      | missing_param    |
      | camera_id        |
      | ra               |
      | dec              |
      | duration         |
      | tolerance_arcsec |
      | max_attempts     |

  Scenario Outline: Rejects out-of-range numeric parameters
    Given a running Alpaca simulator
    And a stub plate solver returning a canned WCS
    And rp is running with a camera and a mount on the simulator
    And the mount tracking is set to true
    And an MCP client connected to rp
    When the MCP client calls center_on_target with override "<field>" set to <value>
    Then the tool call should return an error
    And the error message should contain "<field>"

    Examples:
      | field            | value |
      | tolerance_arcsec | 0     |
      | max_attempts     | 0     |
      | max_attempts     | 51    |
      | ra               | 24    |
      | dec              | 91    |
