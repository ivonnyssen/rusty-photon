Feature: ACME certificate setup via doctor tls issue --acme
  doctor tls issue --acme requests certificates from Let's Encrypt via
  DNS-01 challenge validation. ACME mode requires --domain,
  --dns-provider, --dns-token, and --email; account state persists to
  acme.json beside the service configs (the config root, not the pki
  directory), so a later renewal run can pick it up without re-passing
  every flag.

  Scenario: tls issue --acme fails without --domain
    When I run doctor tls issue with --acme but no --domain
    Then the command exits with a non-zero status
    And stderr contains "domain"

  Scenario: tls issue --acme fails without --dns-provider
    When I run doctor tls issue with --acme and --domain but no --dns-provider
    Then the command exits with a non-zero status
    And stderr contains "dns-provider"

  Scenario: tls issue --acme fails without --email
    When I run doctor tls issue with --acme and --domain and --dns-provider but no --email
    Then the command exits with a non-zero status
    And stderr contains "email"

  Scenario: tls issue --acme saves the ACME configuration beside the configs
    When I run doctor tls issue with --acme and all required flags pointing to staging
    Then the config root contains "acme.json"
    And "acme.json" contains the provided domain
    And "acme.json" contains the provided email
    And "acme.json" contains the DNS provider name
    And "acme.json" has staging set to true

  Scenario: tls issue without --acme still generates a self-signed CA
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    When I run doctor tls issue
    Then the pki file "ca.pem" exists
    And the pki file "ppba-driver.pem" exists
