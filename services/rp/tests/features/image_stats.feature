Feature: Image statistics tool
  The compute_image_stats MCP tool reads a FITS file from disk and computes
  pixel statistics: median, mean, min, and max ADU values. If a document_id
  is provided, it updates the exposure document with an "image_stats" section.
  This tool does not access the camera -- it operates on saved image files.

  Scenario: Returns median and mean ADU after capture
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 100 ms
    And the MCP client calls "compute_image_stats" with the captured image path
    Then the image stats result should contain "median_adu" as a non-negative integer
    And the image stats result should contain "mean_adu" as a non-negative number
    And the image stats result should contain "min_adu"
    And the image stats result should contain "max_adu"
    And the image stats result should contain "pixel_count" as a positive integer

  Scenario: Tool catalog includes compute_image_stats
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "compute_image_stats"

  Scenario: compute_image_stats with nonexistent image path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_image_stats" with image path "/nonexistent/image.fits"
    Then the tool call should return an error

  Scenario: compute_image_stats with missing image_path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_image_stats" with no image_path
    Then the tool call should return an error
    And the error message should contain "missing image_path"
