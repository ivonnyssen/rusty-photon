@serial
Feature: plate-solver TLS and HTTP Basic Auth
  plate-solver serves HTTPS and requires HTTP Basic Auth when `server.tls`
  and `server.auth` are configured. Absent both, it serves plain
  unauthenticated HTTP. The deep TLS/auth behavior suite for the shared
  server stack lives in ppba-driver; this smoke scenario proves
  plate-solver threads the shared server config into its own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for plate-solver
    And plate-solver is configured with TLS and auth enabled and mock astap
    When plate-solver is started with TLS and auth
    Then the health endpoint should reject missing credentials with 401
    And the health endpoint should respond with valid credentials
