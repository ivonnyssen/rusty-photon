Feature: Sigma-clipped background estimation tool
  The estimate_background MCP tool returns sigma-clipped mean, stddev, and
  median of the image background. It accepts either document_id (resolved via
  the image cache, falling back to FITS on miss) or image_path (read from
  disk). Optional k and max_iters control the clipping kernel; defaults are
  k=3.0 and max_iters=5. When called with document_id, results are written
  into the exposure document as a "background" section -- separate from
  measure_basic's "image_analysis" section so the two tools coexist.

  Scenario: Tool catalog includes estimate_background
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "estimate_background"

  Scenario: Returns contract fields after capture using image_path
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured image path
    Then the estimate_background result should contain "mean" as a non-negative number
    And the estimate_background result should contain "stddev" as a non-negative number
    And the estimate_background result should contain "median" as a non-negative number
    And the estimate_background result should contain "pixel_count" as a positive integer

  Scenario: Returns contract fields when called with document_id
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured document_id
    Then the estimate_background result should contain "mean" as a non-negative number
    And the estimate_background result should contain "stddev" as a non-negative number
    And the estimate_background result should contain "median" as a non-negative number

  Scenario: Persists background section into the exposure document
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured document_id
    And I fetch the exposure document for the captured document_id
    Then the exposure document should contain a section named "background"
    And the "background" section should contain "mean"
    And the "background" section should contain "stddev"
    And the "background" section should contain "median"

  Scenario: Custom k and max_iters override defaults
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured image path, k 2.5 and max_iters 10
    Then the estimate_background result should contain "mean" as a non-negative number
    And the estimate_background result should contain "stddev" as a non-negative number

  Scenario: estimate_background with nonexistent image path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "estimate_background" with image path "/nonexistent/image.fits"
    Then the tool call should return an error

  Scenario: estimate_background with neither document_id nor image_path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "estimate_background" with no arguments
    Then the tool call should return an error
    And the error message should contain "image_path"

  Scenario: estimate_background with unknown document_id returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "estimate_background" with document_id "unknown-doc-id"
    Then the tool call should return an error

  Scenario: estimate_background with non-positive k returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured image path and k 0.0
    Then the tool call should return an error
    And the error message should contain "k"

  Scenario: estimate_background with zero max_iters returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "estimate_background" with the captured image path and max_iters 0
    Then the tool call should return an error
    And the error message should contain "max_iters"
