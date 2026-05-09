@serial
Feature: Image statistics tool
  The compute_image_stats MCP tool computes pixel statistics -- median, mean,
  min, and max ADU values -- on a captured image. It accepts either
  document_id (resolved via the image cache, falling back to FITS on miss)
  or image_path (read from disk via rp-fits). When called with document_id,
  results are written into the exposure document as an "image_stats" section.
  This tool does not access the camera -- it operates on saved image files.

  Scenario: Returns median and mean ADU after capture
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_image_stats" with the captured image path
    Then the image stats result should contain "median_adu" as a non-negative integer
    And the image stats result should contain "mean_adu" as a non-negative number
    And the image stats result should contain "min_adu"
    And the image stats result should contain "max_adu"
    And the image stats result should contain "pixel_count" as a positive integer

  Scenario: Returns contract fields when called with document_id
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_image_stats" with the captured document_id
    Then the image stats result should contain "median_adu" as a non-negative integer
    And the image stats result should contain "mean_adu" as a non-negative number
    And the image stats result should contain "pixel_count" as a positive integer

  Scenario: Persists image_stats section into the exposure document
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_image_stats" with the captured document_id
    And I fetch the exposure document for the captured document_id
    Then the exposure document should contain a section named "image_stats"
    And the "image_stats" section should contain "median_adu"
    And the "image_stats" section should contain "mean_adu"
    And the "image_stats" section should contain "min_adu"
    And the "image_stats" section should contain "max_adu"
    And the "image_stats" section should contain "pixel_count"

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

  Scenario: compute_image_stats with unknown document_id returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_image_stats" with document_id "unknown-doc-id"
    Then the tool call should return an error

  Scenario: compute_image_stats with neither document_id nor image_path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_image_stats" with no arguments
    Then the tool call should return an error
    And the error message should contain "image_path"
