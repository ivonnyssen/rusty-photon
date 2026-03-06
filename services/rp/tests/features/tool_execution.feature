Feature: MCP tool execution against equipment
  rp exposes equipment operations as MCP tools. Workflow plugins call
  tools via the MCP server endpoint. Each tool validates parameters,
  calls the Alpaca device, and returns a structured result.

  Scenario: Capture returns image path and document id
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1 second
    Then the tool result should contain an image path
    And the tool result should contain a document id

  Scenario: Set filter changes the active filter
    Given a running Alpaca simulator
    And rp is running with a filter wheel on the simulator
    And an MCP client connected to rp
    When the MCP client calls "set_filter" with filter wheel "main-fw" and filter "Red"
    And the MCP client calls "get_filter" with filter wheel "main-fw"
    Then the current filter should be "Red"

  Scenario: Capture with invalid camera id returns an error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "nonexistent" for 1 second
    Then the tool call should return an error

  Scenario: Tool catalog includes capture and filter tools
    Given a running Alpaca simulator
    And rp is running with a camera and filter wheel on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "capture"
    And the tool list should include "set_filter"
    And the tool list should include "get_filter"
