@serial
Feature: zwo-focuser TLS and HTTP Basic Auth
  zwo-focuser serves HTTPS and requires HTTP Basic Auth when `server.tls` and
  `server.auth` are configured. Absent both, it serves plain unauthenticated
  HTTP. The deep TLS/auth behavior suite for the shared Alpaca driver stack
  lives in ppba-driver; this smoke scenario proves zwo-focuser threads the
  shared server config into its own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for zwo-focuser
    And zwo-focuser is configured with TLS and auth enabled on the simulation backend
    When zwo-focuser is started with TLS and auth
    Then the Alpaca management endpoint should reject missing credentials with 401
    And the Alpaca management endpoint should respond with valid credentials
