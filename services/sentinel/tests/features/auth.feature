@serial
Feature: sentinel dashboard HTTP Basic Auth
  The sentinel dashboard can require authentication when auth is configured.

  Scenario: dashboard auth enabled with correct credentials returns 200
    Given generated TLS certificates for sentinel
    And sentinel is configured with dashboard TLS and auth enabled
    When sentinel is started with dashboard auth
    Then the dashboard health endpoint should respond with valid credentials

  Scenario: dashboard auth enabled with wrong credentials returns 401
    Given generated TLS certificates for sentinel
    And sentinel is configured with dashboard TLS and auth enabled
    When sentinel is started with dashboard auth
    Then the dashboard health endpoint should reject wrong credentials with 401

  Scenario: dashboard auth enabled with missing credentials returns 401
    Given generated TLS certificates for sentinel
    And sentinel is configured with dashboard TLS and auth enabled
    When sentinel is started with dashboard auth
    Then the dashboard health endpoint should reject missing credentials with 401

  Scenario: dashboard 401 response includes WWW-Authenticate header
    Given generated TLS certificates for sentinel
    And sentinel is configured with dashboard TLS and auth enabled
    When sentinel is started with dashboard auth
    Then the dashboard 401 response should include a WWW-Authenticate header

  Scenario: dashboard auth disabled requires no credentials
    When sentinel is started without dashboard auth
    Then the dashboard health endpoint should respond without credentials

  Scenario: sentinel polls auth-enabled filemonitor with correct credentials
    Given generated TLS certificates for sentinel
    And filemonitor is running with TLS and auth enabled and a contains rule "SAFE" as safe
    And sentinel is configured to monitor the auth-enabled filemonitor
    When sentinel is started with CA trust
    Then sentinel should successfully poll the filemonitor
