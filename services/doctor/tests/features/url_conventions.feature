Feature: URL convention diagnosis
  The Alpaca clients in this stack — rp's equipment[*].alpaca_url and
  ui-htmx's drivers[*].base_url — append /api/v1 themselves, so a configured
  URL must NOT carry it: a doubled prefix resolves nothing and 404s every
  request while leaving the config file individually valid. Only a
  cross-convention check catches it; it is a warning because a deliberate
  reverse proxy could legitimately rewrite paths. (Sentinel's URLs are all
  derived since D3s, so there is no sentinel-side convention left to check.)

  Background:
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-sentinel    |
      | rusty-photon-rp          |
      | rusty-photon-ui-htmx     |
      | rusty-photon-qhy-focuser |

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
