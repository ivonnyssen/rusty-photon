Feature: Restart via Sentinel
  Sentinel owns process restart; the BFF is its authorised client. When the
  BFF config carries a sentinel block, every driver's config card renders a
  "Restart via Sentinel" button posting to /config/{service}/restart; the BFF
  forwards to Sentinel's POST /api/services/{name}/restart (the name is the
  driver's sentinel_service, defaulting to its service id) and renders the
  outcome. An accepted restart swaps in the reconnecting fragment, which
  polls /config/{service}/status until the driver serves its configuration
  again; a failed restart command surfaces Sentinel's failure detail; a
  service Sentinel does not supervise surfaces Sentinel's not-found reason.
  Without a sentinel block, no restart affordance is rendered.

  Scenario: The config card offers a restart via Sentinel
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel supervising "dsd-fp2" with a restart command that writes a marker file
    When I open the dsd-fp2 config page
    Then the page offers to restart the driver via Sentinel

  Scenario: An accepted restart runs Sentinel's command and the page reconnects
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel supervising "dsd-fp2" with a restart command that writes a marker file
    When I request a restart of the dsd-fp2 driver
    Then the restart marker file exists
    And the page reports the driver is restarting
    And the page polls /config/dsd-fp2/status every 1s for reconnection
    When I poll the reconnect status until max_brightness 4096 is served
    Then the page shows the value "4096"

  Scenario: A failing restart command surfaces Sentinel's failure detail
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel supervising "dsd-fp2" with a restart command that fails
    When I request a restart of the dsd-fp2 driver
    Then the page reports the restart failed

  Scenario: A driver Sentinel does not supervise surfaces the not-found reason
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel supervising no services
    When I request a restart of the dsd-fp2 driver
    Then the page reports Sentinel does not supervise the driver

  Scenario: No sentinel configured renders no restart affordance
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I open the dsd-fp2 config page
    Then the page offers no restart affordance
