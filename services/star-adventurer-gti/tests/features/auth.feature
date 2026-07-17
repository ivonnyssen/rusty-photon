@serial
Feature: star-adventurer-gti TLS and HTTP Basic Auth
  star-adventurer-gti serves HTTPS and requires HTTP Basic Auth when
  `server.tls` and `server.auth` are configured. Absent both, it serves
  plain unauthenticated HTTP. The deep TLS/auth behavior suite for the
  shared Alpaca driver stack lives in ppba-driver; this smoke scenario
  proves star-adventurer-gti threads the shared server config into its
  own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for star-adventurer-gti
    And star-adventurer-gti is configured with TLS and auth enabled and mock serial
    When star-adventurer-gti is started with TLS and auth
    Then the Alpaca management endpoint should reject missing credentials with 401
    And the Alpaca management endpoint should respond with valid credentials
