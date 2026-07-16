@serial
Feature: phd2-guider TLS and HTTP Basic Auth
  phd2-guider serves HTTPS and requires HTTP Basic Auth when `server.tls`
  and `server.auth` are configured. Absent both, it serves plain
  unauthenticated HTTP. The deep TLS/auth behavior suite for the shared
  server stack lives in ppba-driver; this smoke scenario proves
  phd2-guider threads the shared server config into its own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given a mock PHD2 that settles successfully
    And generated TLS certificates for phd2-guider
    And phd2-guider is configured with TLS and auth enabled pointing at the mock PHD2
    When phd2-guider is started with TLS and auth
    Then the health endpoint should reject missing credentials with 401
    And the health endpoint should respond with valid credentials
