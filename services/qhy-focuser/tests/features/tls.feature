Feature: qhy-focuser TLS support
  qhy-focuser can serve over HTTPS when TLS is configured.

  Scenario: qhy-focuser starts with TLS and accepts HTTPS requests
    Given generated TLS certificates for qhy-focuser
    And qhy-focuser is configured with TLS enabled and mock serial
    When qhy-focuser is started with TLS
    Then the Alpaca management endpoint should respond over HTTPS
