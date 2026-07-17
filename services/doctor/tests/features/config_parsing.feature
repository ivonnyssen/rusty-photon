Feature: Config file parsing diagnosis
  Doctor parses each catalog service's <svc>.json the way the service itself
  would: the file must be valid JSON, and its top-level "server" block must
  parse under the catalog-declared shared shape — AlpacaServerConfig for the
  Alpaca drivers, ServerConfig for the core services — including
  deny_unknown_fields. A file the service would refuse to start on is a fail
  before the next night instead of at it. An absent server block is ok: the
  service applies its own defaults. Everything outside the server block and
  the known cross-reference blocks is opaque to doctor, so a typo there is
  invisible until D5's per-service validation — by design.

  Background:
    Given platform facts with an enabled unit "rusty-photon-qhy-focuser"

  Scenario: Invalid JSON is a fail
    Given a config directory where "qhy-focuser.json" is not valid JSON
    When I run doctor with --json
    Then the report contains a "fail" check named "config.json-syntax" for service "qhy-focuser"

  Scenario: An unknown key in the server block is a fail
    Given a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113, "prot": 2 } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.server-shape" for service "qhy-focuser"
    And that check's detail mentions "prot"

  Scenario: A server block present without a port is a fail
    Given a config directory with "qhy-focuser.json" containing:
      """
      { "server": {} }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.server-shape" for service "qhy-focuser"

  Scenario: discovery_port on a core service is a fail
    Given platform facts with an enabled unit "rusty-photon-ui-htmx"
    And a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11120, "discovery_port": 32227 } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.server-shape" for service "ui-htmx"
    And that check's detail mentions "discovery_port"

  Scenario: A malformed bind_address is a fail
    Given a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113, "bind_address": "localhost" } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.server-shape" for service "qhy-focuser"

  Scenario: An absent server block is ok — the service defaults apply
    Given a config directory with "qhy-focuser.json" containing:
      """
      {}
      """
    When I run doctor with --json
    Then the report contains an "ok" check named "config.server-shape" for service "qhy-focuser"

  Scenario: An unparseable known cross-reference block is a fail
    Given platform facts with an enabled unit "rusty-photon-sentinel"
    And a config directory with "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "operation_watchdog": { "operations": [ "not", "a", "map" ] } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "config.known-blocks" for service "sentinel"
