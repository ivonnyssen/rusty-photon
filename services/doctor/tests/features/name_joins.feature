Feature: Cross-config name join diagnosis
  One service name is spelled in up to four places today — the ui-htmx
  drivers key, its sentinel_service field, sentinel's services key, and the
  watchdog's operations.<family>.service — matched by convention and
  validated by nothing, so a mismatch surfaces at 2am as a 404 in a UI
  banner. Doctor validates every join it can see on one host: sentinel
  restart_commands must name units the service manager reports (the two
  historical rot patterns — a --user scope against system units, and a unit
  name missing the rusty-photon- prefix — are called out by name), watchdog
  operations must reference keys of sentinel's services map, ui-htmx's
  sentinel_service values must too, and a ui-htmx driver keyed by a catalog
  service and pointing at localhost must use that service's effective port.

  Background:
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-sentinel    |
      | rusty-photon-qhy-focuser |
      | rusty-photon-ui-htmx     |

  Scenario: A restart_command naming an unknown unit is a dangling join
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focusser" } } }
      """
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "joins.sentinel-unit" for service "sentinel"
    And that check's detail mentions "rusty-photon-qhy-focusser"

  Scenario: A --user restart_command against a system unit is called out
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl --user restart rusty-photon-qhy-focuser" } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.sentinel-unit" for service "sentinel"
    And that check's detail mentions "--user"

  Scenario: A restart_command missing the rusty-photon- prefix is called out
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart qhy-focuser" } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.sentinel-unit" for service "sentinel"
    And that check's suggestion mentions "rusty-photon-qhy-focuser"

  Scenario: A restart_command naming an installed unit resolves
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    When I run doctor with --json
    Then the report contains an "ok" check named "joins.sentinel-unit" for service "sentinel"

  Scenario: A watchdog operation naming a service absent from the services map dangles
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focuser" } },
        "operation_watchdog": {
          "rp_url": "http://localhost:11115",
          "operations": { "slew": { "service": "mount" } } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.watchdog-service" for service "sentinel"
    And that check's detail mentions "mount"

  Scenario: A ui-htmx sentinel_service absent from sentinel's services map dangles
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113", "sentinel_service": "qhy-focuser" } },
        "sentinel": { "base_url": "http://localhost:11114" } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.ui-htmx-sentinel" for service "ui-htmx"
    And that check's detail mentions "qhy-focuser"

  Scenario: A ui-htmx driver defaulting its sentinel_service to a present key resolves
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "qhy-focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113" } },
        "sentinel": { "base_url": "http://localhost:11114" } }
      """
    When I run doctor with --json
    Then the report contains an "ok" check named "joins.ui-htmx-sentinel" for service "ui-htmx"

  Scenario: Without a sentinel target ui-htmx sentinel joins are not checked
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113", "sentinel_service": "ghost" } } }
      """
    When I run doctor with --json
    Then the report has no checks named "joins.ui-htmx-sentinel"

  Scenario: A localhost driver URL on the wrong port is the 2am 404, caught at noon
    Given a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11114" } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.ui-htmx-driver-port" for service "ui-htmx"
    And that check's detail mentions "11113"

  Scenario: A non-localhost driver URL is out of one-host scope
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://10.0.85.245:9999" } } }
      """
    When I run doctor with --json
    Then the report has no checks named "joins.ui-htmx-driver-port" with status "fail"
