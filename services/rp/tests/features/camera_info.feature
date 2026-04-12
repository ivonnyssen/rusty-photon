Feature: Camera info tool
  The get_camera_info MCP tool reads camera capabilities from the connected
  ASCOM Alpaca device. It returns max_adu (full well depth in ADU),
  exposure time limits, and sensor dimensions. Workflow plugins use this
  to compute target ADU levels for flat calibration.

  Scenario: Returns max_adu and sensor dimensions for connected camera
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_camera_info" with camera "main-cam"
    Then the tool result should contain "max_adu" as a positive integer
    And the tool result should contain "sensor_x" as a positive integer
    And the tool result should contain "sensor_y" as a positive integer

  Scenario: Returns exposure limits for connected camera
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_camera_info" with camera "main-cam"
    Then the tool result should contain "exposure_min_ms"
    And the tool result should contain "exposure_max_ms"

  Scenario: Tool catalog includes get_camera_info
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "get_camera_info"

  Scenario: get_camera_info with nonexistent camera returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_camera_info" with camera "nonexistent"
    Then the tool call should return an error
    And the error message should contain "camera not found"

  Scenario: get_camera_info with disconnected camera returns error
    Given rp is running with a camera at "http://localhost:1" device 0
    And an MCP client connected to rp
    When the MCP client calls "get_camera_info" with camera "main-cam"
    Then the tool call should return an error
    And the error message should contain "camera not connected"

  Scenario: get_camera_info with missing camera_id returns error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_camera_info" with no camera_id
    Then the tool call should return an error
    And the error message should contain "camera_id"
