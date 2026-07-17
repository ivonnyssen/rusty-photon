Feature: Port collision diagnosis
  Every service's effective port is its configured server.port, or its
  catalog default when the file or the server block is absent. A service
  participates in collision checking when its unit is installed or its
  config file exists. Two services resolving to the same effective TCP port
  is a fail — one of them will not bind. Two Alpaca configs enabling the
  same discovery_port is likewise a fail: the UDP responder is a per-host
  opt-in for single-driver deployments precisely because multiple responders
  collide.

  Scenario: Two configured services on the same port collide
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-qhy-focuser |
      | rusty-photon-dsd-fp2     |
    And a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "ports.collision"
    And that check's detail mentions "11113"
    And that check's detail mentions "qhy-focuser"
    And that check's detail mentions "dsd-fp2"

  Scenario: A configured port colliding with another service's catalog default is caught
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-qhy-focuser |
      | rusty-photon-dsd-fp2     |
    And a config directory with "dsd-fp2.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "ports.collision"
    And that check's detail mentions "qhy-focuser"

  Scenario: Distinct ports do not collide
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-qhy-focuser |
      | rusty-photon-dsd-fp2     |
    And a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11119 } }
      """
    When I run doctor with --json
    Then the report has no checks named "ports.collision" with status "fail"

  Scenario: Services that are neither installed nor configured do not reserve their default port
    Given platform facts with an enabled unit "rusty-photon-ui-htmx"
    And a config directory with "ui-htmx.json" containing:
      """
      { "server": { "port": 11113 } }
      """
    When I run doctor with --json
    Then the report has no checks named "ports.collision" with status "fail"

  Scenario: Two Alpaca drivers enabling the same discovery port collide
    Given platform facts with enabled units:
      | unit                     |
      | rusty-photon-qhy-focuser |
      | rusty-photon-dsd-fp2     |
    And a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113, "discovery_port": 32227 } }
      """
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11119, "discovery_port": 32227 } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "ports.discovery-collision"
    And that check's detail mentions "32227"

  Scenario: A single enabled discovery responder is legal
    Given platform facts with an enabled unit "rusty-photon-qhy-focuser"
    And a config directory with "qhy-focuser.json" containing:
      """
      { "server": { "port": 11113, "discovery_port": 32227 } }
      """
    When I run doctor with --json
    Then the report has no checks named "ports.discovery-collision" with status "fail"
