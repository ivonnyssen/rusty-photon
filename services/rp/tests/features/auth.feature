@serial
Feature: rp HTTP Basic Auth
  rp can require authentication when auth is configured.

  Scenario: auth enabled with correct credentials returns 200
    Given generated TLS certificates
    And rp is configured with TLS and auth enabled
    When rp is started with auth
    Then the health endpoint should respond with valid credentials

  Scenario: auth enabled with wrong credentials returns 401
    Given generated TLS certificates
    And rp is configured with TLS and auth enabled
    When rp is started with auth
    Then the health endpoint should reject wrong credentials with 401

  Scenario: auth enabled with missing credentials returns 401
    Given generated TLS certificates
    And rp is configured with TLS and auth enabled
    When rp is started with auth
    Then the health endpoint should reject missing credentials with 401

  Scenario: 401 response includes WWW-Authenticate header
    Given generated TLS certificates
    And rp is configured with TLS and auth enabled
    When rp is started with auth
    Then the rp 401 response should include a WWW-Authenticate header

  Scenario: auth disabled requires no credentials
    When rp is started without TLS
    Then the health endpoint should respond over HTTP
