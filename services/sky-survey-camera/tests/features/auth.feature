@serial
Feature: sky-survey-camera TLS and HTTP Basic Auth
  sky-survey-camera serves HTTPS and requires HTTP Basic Auth when
  `server.tls` and `server.auth` are configured. Absent both, it serves plain
  unauthenticated HTTP. The deep TLS/auth behavior suite for the shared Alpaca
  driver stack lives in ppba-driver; this smoke scenario proves
  sky-survey-camera threads the shared server config into its own serve path.

  Scenario: TLS with auth rejects missing credentials with 401 and accepts valid ones
    Given generated TLS certificates for sky-survey-camera
    And sky-survey-camera is configured with TLS and auth enabled and a stub survey backend
    When sky-survey-camera is started with TLS and auth
    Then the Alpaca management endpoint should reject missing credentials with 401
    And the Alpaca management endpoint should respond with valid credentials
