@serial
Feature: ui-htmx TLS
  ui-htmx serves HTTPS when `server.tls` names a certificate and key issued
  by the Rusty Photon CA. Absent `server.tls` (the default), it serves plain
  HTTP. TLS is independent of auth: a TLS-only BFF answers without
  credentials.

  Scenario: TLS enabled serves the health endpoint over HTTPS
    Given generated TLS certificates for ui-htmx
    And ui-htmx is configured with TLS enabled and no auth
    When ui-htmx is started with TLS
    Then the health endpoint should answer over HTTPS without credentials
