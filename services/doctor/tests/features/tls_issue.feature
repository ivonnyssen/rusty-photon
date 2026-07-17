@wip
Feature: Certificate issuance via doctor tls issue
  doctor tls issue creates the self-signed CA (if absent) and a
  certificate pair for each installed service that lacks one, under the
  config root's pki directory. The service set is derived from the
  catalog and what is installed — not from a hand-typed default list —
  so services the retired rp_tls DEFAULT_SERVICES list missed (dsd-fp2
  among them) are covered. Configs are never touched; that is the --fix
  provisioning pass.

  Scenario: tls issue generates the CA and certificates for every installed service
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11119 } }
      """
    When I run doctor tls issue
    Then the pki file "ca.pem" exists
    And the pki file "ca-key.pem" exists
    And the pki file "ppba-driver.pem" exists
    And the pki file "dsd-fp2.pem" exists
    And the config file "ppba-driver.json" is unchanged from what was staged
    And the config file "dsd-fp2.json" is unchanged from what was staged

  Scenario: tls issue preserves the existing CA on re-run
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor tls issue has already run
    When I run doctor tls issue
    Then the pki file "ca.pem" is unchanged
    And the pki file "ppba-driver.pem" is unchanged

  Scenario: The --services flag limits certificate generation
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And a config file "dsd-fp2.json" containing:
      """
      { "server": { "port": 11119 } }
      """
    When I run doctor tls issue limited to the service "ppba-driver"
    Then the pki file "ppba-driver.pem" exists
    And the pki file "dsd-fp2.pem" does not exist

  Scenario: The --force flag re-issues service certificates but never the CA
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor tls issue has already run
    When I run doctor tls issue with --force
    Then the pki file "ca.pem" is unchanged
    And the pki file "ppba-driver.pem" has changed

  Scenario: Generated certificates are valid for TLS
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And doctor tls issue has already run
    When a test HTTPS server is started with the "ppba-driver" certificate
    And a client connects using the generated CA certificate
    Then the HTTPS connection succeeds
