Feature: Equipment connectivity
  rp connects to ASCOM Alpaca devices at startup and reports their status
  through the REST API. Equipment configuration is read from the config
  file. Connection failures are reported but do not prevent startup.

  Scenario: Camera is connected after startup
    Given a running Alpaca simulator
    And rp is configured with a camera on the simulator
    When rp starts
    Then the equipment status should show the camera as connected

  Scenario: Filter wheel is connected after startup
    Given a running Alpaca simulator
    And rp is configured with a filter wheel on the simulator
    When rp starts
    Then the equipment status should show the filter wheel as connected

  Scenario: Both camera and filter wheel are connected after startup
    Given a running Alpaca simulator
    And rp is configured with a camera on the simulator
    And rp is configured with a filter wheel on the simulator
    When rp starts
    Then the equipment status should show the camera as connected
    And the equipment status should show the filter wheel as connected

  Scenario: Equipment status reports disconnected when simulator is unreachable
    Given rp is configured with a camera at "http://localhost:1" device 0
    When rp starts
    Then the equipment status should show the camera as disconnected

  Scenario: Filter wheel reports disconnected when simulator is unreachable
    Given rp is configured with a filter wheel at "http://localhost:1" device 0
    When rp starts
    Then the equipment status should show the filter wheel as disconnected

  Scenario: Startup succeeds with no equipment configured
    When rp starts
    Then rp should be healthy

  Scenario: Camera on wrong device number reports disconnected
    Given a running Alpaca simulator
    And rp is configured with a camera at the simulator device 99
    When rp starts
    Then the equipment status should show the camera as disconnected

  Scenario: Filter wheel on wrong device number reports disconnected
    Given a running Alpaca simulator
    And rp is configured with a filter wheel at the simulator device 99
    When rp starts
    Then the equipment status should show the filter wheel as disconnected

  Scenario: Mixed reachable and unreachable equipment
    Given a running Alpaca simulator
    And rp is configured with a camera on the simulator
    And rp is configured with a filter wheel at "http://localhost:1" device 0
    When rp starts
    Then the equipment status should show the camera as connected
    And the equipment status should show the filter wheel as disconnected
