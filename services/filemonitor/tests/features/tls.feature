Feature: filemonitor TLS support
  filemonitor can serve over HTTPS when TLS is configured.

  Scenario: filemonitor starts with TLS and accepts HTTPS requests
    Given generated TLS certificates for filemonitor
    And a monitoring file containing "SAFE"
    And a contains rule with pattern "SAFE" that evaluates to safe
    And filemonitor is configured with TLS enabled
    When filemonitor is started with TLS
    Then the Alpaca management endpoint should respond over HTTPS
