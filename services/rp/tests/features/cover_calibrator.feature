@serial
Feature: CoverCalibrator tools
  rp exposes CoverCalibrator device operations as MCP tools. These control
  flat panel light sources and dust covers. close_cover and open_cover
  manage the dust cover. calibrator_on and calibrator_off manage the light
  source. All operations block until the device reaches the target state.

  Scenario: close_cover closes the cover successfully
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "close_cover" with calibrator "flat-panel"
    Then the tool call should succeed

  Scenario: open_cover opens the cover successfully
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "close_cover" with calibrator "flat-panel"
    And the MCP client calls "open_cover" with calibrator "flat-panel"
    Then the tool call should succeed

  Scenario: calibrator_on turns on the light at default brightness
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "calibrator_on" with calibrator "flat-panel"
    Then the tool call should succeed

  Scenario: calibrator_on with explicit brightness succeeds
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "calibrator_on" with calibrator "flat-panel" and brightness 50
    Then the tool call should succeed

  Scenario: calibrator_off turns off the light
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "calibrator_on" with calibrator "flat-panel"
    And the MCP client calls "calibrator_off" with calibrator "flat-panel"
    Then the tool call should succeed

  Scenario: Tool catalog includes CoverCalibrator tools
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "close_cover"
    And the tool list should include "open_cover"
    And the tool list should include "calibrator_on"
    And the tool list should include "calibrator_off"

  Scenario: close_cover with nonexistent calibrator returns error
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "close_cover" with calibrator "nonexistent"
    Then the tool call should return an error
    And the error message should contain "calibrator not found"

  Scenario: close_cover with disconnected calibrator returns error
    Given rp is running with a cover calibrator at "http://localhost:1" device 0
    And an MCP client connected to rp
    When the MCP client calls "close_cover" with calibrator "flat-panel"
    Then the tool call should return an error
    And the error message should contain "calibrator not connected"

  Scenario: close_cover with missing calibrator_id returns error
    Given a running Alpaca simulator
    And rp is running with a cover calibrator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "close_cover" with no calibrator_id
    Then the tool call should return an error
    And the error message should contain "missing calibrator_id"
