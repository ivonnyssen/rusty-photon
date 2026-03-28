Feature: sentinel TLS support
  Sentinel dashboard can serve over HTTPS and sentinel can poll
  TLS-enabled safety monitors.

  @serial
  Scenario: Dashboard starts with TLS and accepts HTTPS requests
    Given generated TLS certificates for sentinel
    And sentinel is configured with dashboard TLS enabled
    When sentinel is started
    Then the dashboard health endpoint should respond over HTTPS

  @serial
  Scenario: Sentinel polls a TLS-enabled filemonitor
    Given generated TLS certificates for sentinel
    And a monitoring file containing "SAFE"
    And filemonitor is running with TLS enabled and a contains rule "SAFE" as safe
    And sentinel is configured with CA certificate and HTTPS monitor scheme
    And sentinel is running with CA trust
    When I wait for sentinel to poll
    Then the dashboard status should show "Safe" for "Roof Monitor"
