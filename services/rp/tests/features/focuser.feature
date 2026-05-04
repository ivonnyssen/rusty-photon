@serial
Feature: Focuser tools
  rp exposes Focuser device operations as MCP tools. move_focuser drives
  the focuser to an absolute position and blocks until the device reports
  is_moving=false. get_focuser_position reads the current absolute
  position. get_focuser_temperature returns the sensor temperature in
  degrees Celsius, or null when the focuser's Temperature property is
  not implemented (i.e. the device returns NOT_IMPLEMENTED). The
  Temperature property is independent of TempCompAvailable: a focuser
  may surface a temperature reading even when temperature compensation
  is unavailable. move_focuser validates the requested position against
  the operator-configured min_position / max_position bounds before
  issuing the Alpaca call.

  # Test position convention: target focuser positions in these
  # scenarios are chosen to land within ~500 steps of OmniSim's
  # default focuser position (25000). At OmniSim's simulated slew
  # rate (~400 steps/sec), a 20k-step move costs ~50s; the BDD
  # before(scenario) hook resets the focuser to the default before
  # every scenario, so a "round number" target like 5000 turns the
  # whole feature into a multi-minute slewfest. See
  # docs/references/omnisim.md (Focuser section). Keep new
  # focuser-motion scenarios near 25000 unless the test specifically
  # cares about the absolute value.

  Scenario: Tool catalog includes Focuser tools
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "move_focuser"
    And the tool list should include "get_focuser_position"
    And the tool list should include "get_focuser_temperature"

  Scenario: move_focuser drives the focuser to an absolute position
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 25100
    Then the tool call should succeed
    And the move_focuser result actual_position should be 25100

  Scenario: move_focuser with target equal to current position succeeds
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 24500
    And the MCP client calls "move_focuser" with focuser "main-focuser" to position 24500
    Then the tool call should succeed
    And the move_focuser result actual_position should be 24500

  Scenario: get_focuser_position reads the current absolute position
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 25500
    And the MCP client calls "get_focuser_position" with focuser "main-focuser"
    Then the tool call should succeed
    And the get_focuser_position result position should be 25500

  Scenario: get_focuser_temperature returns a temperature_c field
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_focuser_temperature" with focuser "main-focuser"
    Then the tool call should succeed
    And the get_focuser_temperature result should contain a "temperature_c" field

  Scenario: move_focuser with nonexistent focuser returns error
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "nonexistent" to position 1000
    Then the tool call should return an error
    And the error message should contain "focuser not found"

  Scenario: move_focuser with disconnected focuser returns error
    Given rp is running with a focuser at "http://localhost:1" device 0
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 1000
    Then the tool call should return an error
    And the error message should contain "focuser not connected"

  Scenario: move_focuser below min_position returns error
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator with bounds 1000..9000
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 500
    Then the tool call should return an error
    And the error message should contain "position out of range"

  Scenario: move_focuser above max_position returns error
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator with bounds 1000..9000
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with focuser "main-focuser" to position 9500
    Then the tool call should return an error
    And the error message should contain "position out of range"

  Scenario: move_focuser with missing focuser_id returns error
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_focuser" with no focuser_id
    Then the tool call should return an error
    And the error message should contain "focuser_id"

  Scenario: get_focuser_position with nonexistent focuser returns error
    Given a running Alpaca simulator
    And rp is running with a focuser on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_focuser_position" with focuser "nonexistent"
    Then the tool call should return an error
    And the error message should contain "focuser not found"
