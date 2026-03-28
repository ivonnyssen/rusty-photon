Feature: rp service TLS support
  rp can serve over HTTPS when TLS is configured.

  Scenario: rp starts with TLS and accepts HTTPS requests
    Given generated TLS certificates
    And rp is configured with TLS enabled
    When rp is started
    Then the health endpoint should respond over HTTPS

  Scenario: rp starts without TLS and accepts HTTP requests
    When rp is started without TLS
    Then the health endpoint should respond over HTTP
