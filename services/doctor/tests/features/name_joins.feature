Feature: Cross-config name join diagnosis
  Since sentinel discovers its services from the platform service manager
  (D3s), a service name is spelled in two places doctor can validate on one
  host: the watchdog's operations.<family>.service and the ui-htmx drivers
  key — both resolve against the installed rusty-photon-* units, matched by
  convention and validated by nothing at runtime until the 2am 404. Doctor
  validates the joins, flags the config keys D3s retired (sentinel's
  services map, ui-htmx's per-driver sentinel_service — either one keeps its
  service from starting), and checks that a ui-htmx driver keyed by a
  catalog service and pointing at localhost uses that service's effective
  port.

  Background:
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-sentinel    |
      | rusty-photon-qhy-focuser |
      | rusty-photon-ui-htmx     |

  Scenario: A retired sentinel services map keeps sentinel from starting
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "focuser": { "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "config.retired-keys" for service "sentinel"
    And that check's suggestion mentions "delete"

  Scenario: A retired ui-htmx sentinel_service field keeps ui-htmx from starting
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113", "sentinel_service": "qhy-focuser" } },
        "sentinel": { "base_url": "http://localhost:11114" } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.retired-keys" for service "ui-htmx"
    And that check's detail mentions "sentinel_service"

  Scenario: A watchdog operation naming an uninstalled service dangles
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "operation_watchdog": {
          "rp_url": "http://localhost:11115",
          "operations": { "slew": { "service": "mount" } } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.watchdog-service" for service "sentinel"
    And that check's detail mentions "mount"
    And that check's suggestion mentions "qhy-focuser"

  Scenario: A watchdog operation naming an installed service resolves
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "operation_watchdog": {
          "rp_url": "http://localhost:11115",
          "operations": { "move_focuser": { "service": "qhy-focuser" } } } }
      """
    When I run doctor with --json
    Then the report has no checks named "joins.watchdog-service"

  Scenario: A ui-htmx driver with no installed unit warns that its restart button 404s
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "third-party-dome": { "base_url": "http://localhost:7843" } },
        "sentinel": { "base_url": "http://localhost:11114" } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "joins.ui-htmx-restart" for service "ui-htmx"
    And that check's detail mentions "third-party-dome"

  Scenario: A ui-htmx driver keyed by an installed service resolves
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113" } },
        "sentinel": { "base_url": "http://localhost:11114" } }
      """
    When I run doctor with --json
    Then the report contains an "ok" check named "joins.ui-htmx-restart" for service "ui-htmx"

  Scenario: Without a sentinel target ui-htmx restart joins are not checked
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "third-party-dome": { "base_url": "http://localhost:7843" } } }
      """
    When I run doctor with --json
    Then the report has no checks named "joins.ui-htmx-restart"

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
