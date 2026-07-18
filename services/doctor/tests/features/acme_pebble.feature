@wip
Feature: End-to-end ACME issuance and renewal against Pebble
  These scenarios drive the real instant-acme order flow — account,
  order, DNS-01 challenge, finalize, download — against Pebble (Let's
  Encrypt's official ACME test server) with its pebble-challtestsrv DNS
  sidecar answering the validation queries. Each scenario runs a private
  Pebble on dynamic ports whose HTTPS endpoint uses a test-minted
  certificate, and reaches it through the production knobs an internal
  ACME directory would use: acme.json's directory_url and acme_root.
  The scenarios are tagged pebble and run when PEBBLE_PATH and
  PEBBLE_CHALLTESTSRV_PATH are set (CI always sets them; locally the
  suite announces the skip — docs/skills/testing.md 5.6).

  @pebble
  Scenario: tls issue --acme obtains a real wildcard certificate
    Given a local ACME directory issuing certificates valid for 90 days
    When I run doctor tls issue --acme against the local directory for domain "observatory.test"
    Then the command exits with status 0
    And the pki file "acme-cert.pem" exists
    And the pki file "acme-key.pem" exists
    And the certificate "acme-cert.pem" covers "*.observatory.test"
    And the config root contains "acme.json"
    And "acme.json" records the local directory URL

  @pebble
  Scenario: renew outside the renewal window is a no-op
    Given a local ACME directory issuing certificates valid for 90 days
    And doctor tls issue --acme has already run against it for domain "observatory.test"
    When I run doctor tls renew
    Then the command exits with status 0
    And the pki file "acme-cert.pem" is unchanged

  @pebble
  Scenario: renew inside the renewal window renews through the persisted acme.json
    Given a local ACME directory issuing certificates valid for 1 hour
    And doctor tls issue --acme has already run against it for domain "observatory.test"
    When I run doctor tls renew with --json
    Then the command exits with status 0
    And the pki file "acme-cert.pem" has changed
    And the report records an applied "renew-acme" provisioning action

  @pebble
  Scenario: a missing wildcard certificate with acme.json present is recovered by renew
    Given a local ACME directory issuing certificates valid for 90 days
    And doctor tls issue --acme has already run against it for domain "observatory.test"
    And the pki file "acme-cert.pem" has been deleted
    When I run doctor tls renew
    Then the command exits with status 0
    And the pki file "acme-cert.pem" exists

  @pebble
  Scenario: a successful renewal runs the post-renewal hooks
    Given a local ACME directory issuing certificates valid for 1 hour
    And doctor tls issue --acme has already run against it for domain "observatory.test"
    And acme.json is amended with a post-renewal hook that writes a marker file
    When I run doctor tls renew
    Then the command exits with status 0
    And the post-renewal marker file exists

  @pebble
  Scenario: a failing post-renewal hook exits 2 after the renewal itself succeeded
    Given a local ACME directory issuing certificates valid for 1 hour
    And doctor tls issue --acme has already run against it for domain "observatory.test"
    And acme.json is amended with a post-renewal hook that fails
    When I run doctor tls renew
    Then the command exits with status 2
    And the pki file "acme-cert.pem" has changed
