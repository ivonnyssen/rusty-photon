Feature: TLS certificate management
  The rp init-tls command generates a CA and per-service certificates
  for enabling HTTPS across Rusty Photon services.

  Scenario: init-tls generates CA and all default service certificates
    When rp init-tls is executed with a temporary output directory
    Then the CA certificate should exist
    And the CA private key should exist
    And certificates should exist for "filemonitor"
    And certificates should exist for "ppba-driver"
    And certificates should exist for "qhy-focuser"
    And certificates should exist for "rp"
    And certificates should exist for "sentinel"

  Scenario: init-tls preserves existing CA on re-run
    Given rp init-tls has been executed once
    When rp init-tls is executed again with the same output directory
    Then the CA certificate should be unchanged

  Scenario: init-tls with --services flag limits certificate generation
    When rp init-tls is executed with services "rp" and "sentinel"
    Then certificates should exist for "rp"
    And certificates should exist for "sentinel"
    And certificates should not exist for "filemonitor"

  Scenario: Generated certificates are valid for TLS
    Given rp init-tls has been executed
    When a test HTTPS server is started with the rp certificate
    And a client connects using the generated CA certificate
    Then the HTTPS connection should succeed
