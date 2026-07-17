Feature: URL convention diagnosis
  Two URL conventions coexist and mixing them 404s every request. Sentinel's
  services[*].base_url must carry the /api/v1 suffix — its watchdog appends
  Alpaca method paths below it. rp's equipment[*].alpaca_url and ui-htmx's
  drivers[*].base_url must NOT carry it — those clients append it
  themselves, and a doubled prefix resolves nothing. Both mistakes leave
  every config file individually valid, so only a cross-convention check
  catches them; they are warnings because a deliberate reverse proxy could
  legitimately rewrite paths.

  Background:
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-sentinel    |
      | rusty-photon-rp          |
      | rusty-photon-ui-htmx     |
      | rusty-photon-qhy-focuser |

  Scenario: A sentinel base_url without the /api/v1 suffix warns
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "qhy-focuser": {
            "base_url": "http://localhost:11113",
            "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "urls.sentinel-suffix" for service "sentinel"
    And that check's suggestion mentions "/api/v1"

  Scenario: A sentinel base_url with the suffix passes
    Given a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "services": {
          "qhy-focuser": {
            "base_url": "http://localhost:11113/api/v1",
            "restart_command": "systemctl restart rusty-photon-qhy-focuser" } } }
      """
    When I run doctor with --json
    Then the report has no checks named "urls.sentinel-suffix" with status "warn"

  Scenario: An rp equipment alpaca_url carrying /api/v1 warns
    Given a config directory with "rp.json" containing:
      """
      { "server": { "port": 11115 },
        "equipment": {
          "cameras": [ { "alpaca_url": "http://localhost:11121/api/v1" } ] } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "urls.spurious-suffix" for service "rp"

  Scenario: A ui-htmx driver base_url carrying /api/v1 warns
    Given a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "drivers": {
          "qhy-focuser": { "base_url": "http://localhost:11113/api/v1" } } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "urls.spurious-suffix" for service "ui-htmx"

  Scenario: Suffix-free client URLs pass
    Given a config directory with "rp.json" containing:
      """
      { "server": { "port": 11115 },
        "equipment": {
          "cameras": [ { "alpaca_url": "http://localhost:11121" } ] } }
      """
    When I run doctor with --json
    Then the report has no checks named "urls.spurious-suffix" with status "warn"
