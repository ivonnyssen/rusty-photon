@wip
@serial
Feature: Auto-focus compound tool
  The auto_focus MCP tool drives a V-curve focus sweep using move_focuser,
  capture, and measure_basic internally. It captures one frame at each
  position in the grid current_position ± half_width (in step_size
  increments), measures HFR for each via measure_basic, fits a parabola
  weighted by per-frame star_count, and moves the focuser to the fitted
  vertex. The sweep grid is clamped to the operator-supplied
  min_position / max_position bounds — points outside the bounds are
  dropped, not coerced. The tool errors before any motion when input
  parameters are missing or invalid, when devices are unreachable, when
  the clamped grid has fewer than min_fit_points positions, when the
  sweep yields fewer than min_fit_points non-null HFR samples, or when
  the parabolic fit's leading coefficient `a` is non-positive (the
  curve is monotonic or concave-down, with no minimum inside the
  sampled range). auto_focus does not write a section on any single
  exposure document — the per-frame image_analysis section is written
  by the embedded measure_basic call as it normally would be, and the
  compound result is returned via MCP plus a focus_complete event.

  Scenario: Tool catalog includes auto_focus
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "auto_focus"

  Scenario: auto_focus with nonexistent camera returns error
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus with camera "nonexistent" and focuser "main-focuser"
    Then the tool call should return an error
    And the error message should contain "camera not found"

  Scenario: auto_focus with nonexistent focuser returns error
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus with camera "main-cam" and focuser "nonexistent"
    Then the tool call should return an error
    And the error message should contain "focuser not found"

  Scenario: auto_focus with disconnected focuser returns error
    Given rp is running with a camera on the simulator and an unreachable focuser
    And an MCP client connected to rp
    When the MCP client calls auto_focus with camera "main-cam" and focuser "main-focuser"
    Then the tool call should return an error
    And the error message should contain "focuser not connected"

  Scenario: auto_focus with disconnected camera returns error
    Given rp is running with a focuser on the simulator and an unreachable camera
    And an MCP client connected to rp
    When the MCP client calls auto_focus with camera "main-cam" and focuser "main-focuser"
    Then the tool call should return an error
    And the error message should contain "camera not connected"

  Scenario Outline: auto_focus rejects calls missing required parameters
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus omitting "<missing_param>"
    Then the tool call should return an error
    And the error message should contain "<missing_param>"

    Examples:
      | missing_param |
      | camera_id     |
      | focuser_id    |
      | duration      |
      | step_size     |
      | half_width    |
      | min_area      |
      | max_area      |

  Scenario: auto_focus rejects step_size of 0
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus with step_size 0
    Then the tool call should return an error
    And the error message should contain "step_size"

  Scenario: auto_focus rejects half_width of 0
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus with half_width 0
    Then the tool call should return an error
    And the error message should contain "half_width"

  Scenario: auto_focus rejects min_fit_points below 3
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls auto_focus with min_fit_points 2
    Then the tool call should return an error
    And the error message should contain "min_fit_points"

  Scenario: auto_focus rejects sweep grid too small after focuser bounds clamp
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator with bounds 4900..5100
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 5000
    And the MCP client calls auto_focus with focuser "main-focuser" camera "main-cam" duration "100ms" step_size 100 half_width 500 min_area 5 max_area 65536
    Then the tool call should return an error
    And the error message should contain "min_fit_points"

  Scenario: auto_focus completes its sweep and persists per-step image_analysis sections
    Given rp's data_directory is pinned to a fresh tempdir
    And a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 5000
    And the MCP client calls auto_focus with focuser "main-focuser" camera "main-cam" duration "100ms" step_size 100 half_width 200 min_area 5 max_area 65536
    Then 5 FITS files should exist in the pinned data directory
    And every sidecar JSON in the pinned data directory should contain an "image_analysis" section
    And no sidecar JSON in the pinned data directory should contain an "auto_focus" section
