@wip
Feature: Certificate expiry diagnosis
  The tls.expiry check reads each configured server.tls certificate and
  grades its not_after: expired or unparseable is a failure — rustls
  loads an expired certificate cleanly and only clients reject the
  handshake, so without this check the first symptom is every client
  erroring at night — and inside the 30-day renewal window is a warning.
  Suggestion-only: the fix is doctor tls renew on its timer, not --fix.

  Scenario: an expired configured certificate fails the diagnosis
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" that expired 3 days ago
    And a config file "qhy-focuser.json" with a tls block pointing at the "qhy-focuser" pair
    When I run doctor
    Then the command exits with status 1
    And the report has a "fail" check named "tls.expiry" for service "qhy-focuser"
    And the "tls.expiry" suggestion mentions "doctor tls renew"

  Scenario: a certificate inside the renewal window warns
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    And a config file "qhy-focuser.json" with a tls block pointing at the "qhy-focuser" pair
    When I run doctor
    Then the command exits with status 0
    And the report has a "warn" check named "tls.expiry" for service "qhy-focuser"

  Scenario: a healthy certificate passes the expiry check
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 300 days
    And a config file "qhy-focuser.json" with a tls block pointing at the "qhy-focuser" pair
    When I run doctor
    Then the report has an "ok" check named "tls.expiry" for service "qhy-focuser"

  Scenario: an unparseable certificate file fails the expiry check
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 300 days
    And the pki file "qhy-focuser.pem" is overwritten with garbage
    And a config file "qhy-focuser.json" with a tls block pointing at the "qhy-focuser" pair
    When I run doctor
    Then the command exits with status 1
    And the report has a "fail" check named "tls.expiry" for service "qhy-focuser"
