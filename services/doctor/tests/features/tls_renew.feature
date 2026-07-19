Feature: One-shot certificate renewal via doctor tls renew
  doctor tls renew is the one-shot a platform scheduler runs daily: it
  re-issues, from the existing CA, every self-signed service pair in the
  pki tree whose not_after is within 30 days, and no-ops otherwise. SANs
  carried by the old certificate survive the re-issue (an --extra-san
  name given at issue time is not lost to a renewal that never saw the
  flag), and the CA itself is never renewed — replacing it invalidates
  every distributed trust anchor, so a CA inside its window only earns a
  warning. Services pick a renewed pair up in-process: the TLS acceptor
  re-stats the cert files on handshake (throttled) and swaps the pair
  without a restart. The ACME leg of renewal is specified in
  acme_pebble.feature.

  Scenario: renew on a healthy pki tree is a no-op
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor tls issue has already run
    When I run doctor tls renew
    Then the command exits with status 0
    And the pki file "ppba-driver.pem" is unchanged
    And the pki file "ca.pem" is unchanged

  Scenario: renew re-issues a pair inside the renewal window
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    When I run doctor tls renew
    Then the command exits with status 0
    And the pki file "qhy-focuser.pem" has changed
    And the pki file "ca.pem" is unchanged
    And the certificate "qhy-focuser.pem" is not within its renewal window

  Scenario: renew re-issues an already-expired pair
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" that expired 3 days ago
    When I run doctor tls renew
    Then the command exits with status 0
    And the pki file "qhy-focuser.pem" has changed
    And the certificate "qhy-focuser.pem" is not within its renewal window

  Scenario: renew preserves the old certificate's extra SANs
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days with the extra SAN "observatory.local"
    When I run doctor tls renew
    Then the pki file "qhy-focuser.pem" has changed
    And the certificate "qhy-focuser.pem" carries the SAN "observatory.local"

  Scenario: renew leaves a healthy pair alone while renewing a due one
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    And a certificate pair for "ppba-driver" expiring in 300 days
    When I run doctor tls renew
    Then the pki file "qhy-focuser.pem" has changed
    And the pki file "ppba-driver.pem" is unchanged

  Scenario: renew warns about a CA inside its window but never touches it
    Given a pki tree with a CA expiring in 10 days
    And a certificate pair for "qhy-focuser" expiring in 300 days
    When I run doctor tls renew
    Then the command exits with status 0
    And stderr contains "ca.pem"
    And the pki file "ca.pem" is unchanged

  Scenario: renew reports re-issued pairs as JSON
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    When I run doctor tls renew with --json
    Then the report records an applied "generate-cert" provisioning action for service "qhy-focuser"

  Scenario: renew with --force re-issues a healthy pair
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor tls issue has already run
    When I run doctor tls renew with --force
    Then the command exits with status 0
    And the pki file "ppba-driver.pem" has changed
    And the pki file "ca.pem" is unchanged

  Scenario: renew fails loudly when a due pair has no CA key to re-issue from
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    And the pki file "ca-key.pem" has been deleted
    When I run doctor tls renew
    Then the command exits with status 2
    And stderr contains "ca-key.pem"

  Scenario: a renewed pair is served without restarting the server
    Given a pki tree with a CA
    And a certificate pair for "qhy-focuser" expiring in 5 days
    And a hot-reloading test HTTPS server is started with the "qhy-focuser" certificate
    When I run doctor tls renew
    And a client connects using the generated CA certificate
    Then the HTTPS connection succeeds
    And the server is now serving a different certificate than before the renewal
