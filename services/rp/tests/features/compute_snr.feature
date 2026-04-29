Feature: Signal-to-noise summary tool
  The compute_snr MCP tool runs detect_stars internally and returns the
  median per-star signal-to-noise ratio via the CCD-equation
  approximation: noise = sqrt(signal + n_pixels · sigma_bg^2). It accepts
  either document_id (resolved via the image cache, falling back to FITS
  on miss) or image_path (read from disk). When called with document_id,
  results are written into the exposure document as an "snr" section --
  distinct from the other imaging tools so all five coexist on one
  document.

  Scenario: Tool catalog includes compute_snr
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "compute_snr"

  Scenario: Returns contract fields after capture using image_path
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured image path
    Then the compute_snr result should contain "snr"
    And the compute_snr result should contain "signal"
    And the compute_snr result should contain "noise"
    And the compute_snr result should contain "star_count" as a non-negative integer
    And the compute_snr result should contain "background_mean" as a non-negative number
    And the compute_snr result should contain "background_stddev" as a non-negative number

  Scenario: Returns contract fields when called with document_id
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured document_id
    Then the compute_snr result should contain "snr"
    And the compute_snr result should contain "signal"
    And the compute_snr result should contain "noise"
    And the compute_snr result should contain "star_count" as a non-negative integer

  Scenario: Persists snr section into the exposure document
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured document_id
    And I fetch the exposure document for the captured document_id
    Then the exposure document should contain a section named "snr"
    And the "snr" section should contain "snr"
    And the "snr" section should contain "signal"
    And the "snr" section should contain "noise"
    And the "snr" section should contain "star_count"
    And the "snr" section should contain "background_mean"

  Scenario: Very high threshold yields zero stars with null SNR
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured image path and threshold_sigma 1000.0
    Then the compute_snr result should contain "star_count" with value 0
    And the compute_snr result should contain "snr" with value null
    And the compute_snr result should contain "signal" with value null
    And the compute_snr result should contain "noise" with value null
    And the compute_snr result should contain "background_mean" as a non-negative number

  Scenario: compute_snr with nonexistent image path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_snr" with image path "/nonexistent/image.fits"
    Then the tool call should return an error

  Scenario: compute_snr with neither document_id nor image_path returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_snr" with no arguments
    Then the tool call should return an error
    And the error message should contain "image_path"

  Scenario: compute_snr with unknown document_id returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "compute_snr" with document_id "unknown-doc-id"
    Then the tool call should return an error

  Scenario: compute_snr with image_path but missing min_area returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured image path but no min_area
    Then the tool call should return an error
    And the error message should contain "min_area"

  Scenario: compute_snr with image_path but missing max_area returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And the MCP client calls "compute_snr" with the captured image path but no max_area
    Then the tool call should return an error
    And the error message should contain "max_area"
