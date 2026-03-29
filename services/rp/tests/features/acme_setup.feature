Feature: ACME certificate setup
  The rp init-tls command supports --acme mode to request certificates
  from Let's Encrypt via DNS-01 challenge validation. ACME mode requires
  --domain, --dns-provider, --dns-token, and --email flags.

  Scenario: init-tls --acme fails without --domain
    When rp init-tls is run with --acme but no --domain
    Then the command exits with a non-zero status
    And stderr contains "domain"

  Scenario: init-tls --acme fails without --dns-provider
    When rp init-tls is run with --acme --domain but no --dns-provider
    Then the command exits with a non-zero status
    And stderr contains "dns-provider"

  Scenario: init-tls --acme fails without --email
    When rp init-tls is run with --acme --domain --dns-provider but no --email
    Then the command exits with a non-zero status
    And stderr contains "email"

  Scenario: init-tls --acme saves ACME configuration file
    When rp init-tls is run with --acme and all required flags pointing to staging
    Then acme.json exists in the output directory
    And acme.json contains the provided domain
    And acme.json contains the provided email
    And acme.json contains the DNS provider name
    And acme.json has staging set to true

  Scenario: init-tls without --acme still generates self-signed CA
    When rp init-tls is run without --acme
    Then ca.pem exists in the output directory
    And service certificates exist for default services
