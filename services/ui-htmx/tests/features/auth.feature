@serial
Feature: ui-htmx HTTP Basic Auth
  ui-htmx requires HTTP Basic Auth on every route when `server.auth` is
  configured — credentials are checked before any page handler runs, so even
  `/health` answers 401 without them, with the standard WWW-Authenticate
  challenge. Absent `server.auth` (the default), no credentials are required.

  Scenario: auth enabled with correct credentials returns 200
    Given generated TLS certificates for ui-htmx
    And ui-htmx is configured with TLS and auth enabled
    When ui-htmx is started with TLS and auth
    Then the health endpoint should respond with valid credentials

  Scenario: auth enabled with wrong credentials returns 401
    Given generated TLS certificates for ui-htmx
    And ui-htmx is configured with TLS and auth enabled
    When ui-htmx is started with TLS and auth
    Then the health endpoint should reject wrong credentials with 401

  Scenario: auth enabled with missing credentials returns 401
    Given generated TLS certificates for ui-htmx
    And ui-htmx is configured with TLS and auth enabled
    When ui-htmx is started with TLS and auth
    Then the health endpoint should reject missing credentials with 401

  Scenario: 401 response includes WWW-Authenticate header
    Given generated TLS certificates for ui-htmx
    And ui-htmx is configured with TLS and auth enabled
    When ui-htmx is started with TLS and auth
    Then the 401 response should include a WWW-Authenticate header

  Scenario: auth disabled requires no credentials
    Given ui-htmx is configured without TLS or auth
    When ui-htmx is started without TLS or auth
    Then the health endpoint should respond without credentials
