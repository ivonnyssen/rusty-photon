@serial
Feature: calibrator-flats TLS and HTTP Basic Auth
  calibrator-flats serves HTTPS and requires HTTP Basic Auth when
  `server.tls` and `server.auth` are configured. Absent both, it serves
  plain unauthenticated HTTP. The deep TLS/auth behavior suite for the
  shared server stack lives with the shared crates; this smoke scenario
  proves calibrator-flats threads the shared server config into its own
  serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for calibrator-flats
    And calibrator-flats is configured with TLS and auth enabled
    When calibrator-flats is started with TLS and auth
    Then the health endpoint should reject missing credentials with 401
    And the health endpoint should respond with valid credentials
