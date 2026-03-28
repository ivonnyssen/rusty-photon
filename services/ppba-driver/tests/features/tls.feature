Feature: ppba-driver TLS support
  ppba-driver can serve over HTTPS when TLS is configured.

  Scenario: ppba-driver starts with TLS and accepts HTTPS requests
    Given generated TLS certificates for ppba-driver
    And ppba-driver is configured with TLS enabled and mock serial
    When ppba-driver is started with TLS
    Then the Alpaca management endpoint should respond over HTTPS
