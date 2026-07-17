Feature: Restart via Sentinel
  Sentinel owns process restart; the BFF is its authorised client. When the
  BFF config carries a sentinel block, every driver's config card renders a
  "Restart via Sentinel" button posting to /config/{service}/restart; the BFF
  forwards to Sentinel's POST /api/services/{name}/restart (the name is the
  driver's own service id, matching the discovered rusty-photon-{name} unit)
  and renders the outcome. Sentinel restarts the discovered unit through the
  platform service manager. An accepted restart swaps in the reconnecting
  fragment, which polls /config/{service}/status until the driver serves its
  configuration again; a restart the service manager fails surfaces
  Sentinel's failure detail; a service Sentinel has not discovered surfaces
  Sentinel's not-found reason. Without a sentinel block, no restart
  affordance is rendered.

  Scenario: The config card offers a restart via Sentinel
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel that has discovered the running "dsd-fp2" unit
    When I open the dsd-fp2 config page
    Then the page offers to restart the driver via Sentinel

  Scenario: An accepted restart restarts the discovered unit and the page reconnects
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel that has discovered the running "dsd-fp2" unit
    When I request a restart of the dsd-fp2 driver
    Then the service manager records a restart of "rusty-photon-dsd-fp2"
    And the page reports the driver is restarting
    And the page polls /config/dsd-fp2/status every 1s for reconnection
    When I poll the reconnect status until max_brightness 4096 is served
    Then the page shows the value "4096"

  Scenario: A restart the service manager fails surfaces Sentinel's failure detail
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel that has discovered the running "dsd-fp2" unit
    And the service manager fails restarts of "rusty-photon-dsd-fp2"
    When I request a restart of the dsd-fp2 driver
    Then the page reports the restart failed

  Scenario: A driver Sentinel has not discovered surfaces the not-found reason
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    And a sentinel that has discovered no services
    When I request a restart of the dsd-fp2 driver
    Then the page reports Sentinel does not supervise the driver

  Scenario: No sentinel configured renders no restart affordance
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I open the dsd-fp2 config page
    Then the page offers no restart affordance
