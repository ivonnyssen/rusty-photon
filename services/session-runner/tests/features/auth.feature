@serial
Feature: session-runner TLS and HTTP Basic Auth
  session-runner serves HTTPS and requires HTTP Basic Auth when `server.tls`
  and `server.auth` are configured. Absent both, it serves plain
  unauthenticated HTTP. The deep TLS/auth behavior suite for the shared
  server stack lives with the shared crates; this smoke scenario proves
  session-runner threads the shared server config into its own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for session-runner
    And session-runner is configured with TLS and auth enabled
    When session-runner is started with TLS and auth
    Then the health endpoint should reject missing credentials with 401
    And the health endpoint should respond with valid credentials
