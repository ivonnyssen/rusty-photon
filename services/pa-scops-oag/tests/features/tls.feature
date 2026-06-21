Feature: pa-scops-oag TLS support
  pa-scops-oag can serve over HTTPS when TLS is configured.

  Scenario: pa-scops-oag starts with TLS and accepts HTTPS requests
    Given generated TLS certificates for pa-scops-oag
    And pa-scops-oag is configured with TLS enabled and mock serial
    When pa-scops-oag is started with TLS
    Then the Alpaca management endpoint should respond over HTTPS
