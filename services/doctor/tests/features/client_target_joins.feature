Feature: Client-target joins (#607)
  A service's config can point a URL at another catalog service —
  ui-htmx's rp/sentinel targets, rp's plate-solver/guider clients,
  sentinel's Alpaca monitors, and sentinel's operation-watchdog rp_url.
  These checks join that URL against the *named* service's own
  server.tls/server.auth: a scheme mismatch, or a self-signed target the
  client has no ca_cert_path for, breaks every request
  (joins.client-transport, fail); a target that requires auth while the
  client carries no working credential 401s every request
  (joins.client-auth, warn). The join only resolves for a loopback host —
  doctor diagnoses one config directory, so a different host names a
  service in a config file doctor cannot see.

  Scenario: A plain-HTTP ui-htmx target against a TLS-on rp is flagged
    Given a config file "rp.json" containing:
      """
      { "server": { "port": 11115, "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" } } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 }, "rp": { "base_url": "http://127.0.0.1:11115" } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.client-transport" for service "ui-htmx"
    And that check's detail mentions "uses http"

  Scenario: --fix rewrites ui-htmx's scheme, CA trust, and credential once rp is provisioned
    Given a config file "rp.json" containing:
      """
      { "server": { "port": 11115 } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 }, "rp": { "base_url": "http://127.0.0.1:11115" } }
      """
    When I run doctor with --fix and --json
    Then the config file "ui-htmx.json" has the string "https://127.0.0.1:11115" at "/rp/base_url"
    And the config file "ui-htmx.json" has "/rp/ca_cert_path" pointing at the pki file "ca.pem"
    And the config file "ui-htmx.json" has the string "observatory" at "/rp/auth/username"

  Scenario: A missing credential against an auth-on target is flagged and fixed by --fix
    Given a config file "rp.json" containing:
      """
      { "server": { "port": 11115, "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" },
                    "auth": { "username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$YWJjZGVmZ2g$aGFuZHNldA" } } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 }, "rp": { "base_url": "https://127.0.0.1:11115" } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "joins.client-auth" for service "ui-htmx"
    And that check's detail mentions "carries no credential"

  Scenario: A present but wrong ui-htmx credential is reported, not rewritten
    Given a config file "rp.json" whose auth hash is of the password "right-password"
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 },
        "rp": { "base_url": "http://127.0.0.1:11115",
                "auth": { "username": "observatory", "password": "wrong-password" } } }
      """
    When I run doctor with --json
    Then the report contains a "warn" check named "joins.client-auth" for service "ui-htmx"
    And that check's detail mentions "does not verify"

  Scenario: --fix rewrites rp's plate-solver scheme, CA trust, and credential once the target is provisioned
    Given a config file "plate-solver.json" containing:
      """
      { "server": { "port": 11131,
                    "auth": { "username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$YWJjZGVmZ2g$aGFuZHNldA" } } }
      """
    And a config file "rp.json" containing:
      """
      { "server": { "port": 11115 }, "plate_solver": { "url": "http://localhost:11131" } }
      """
    When I run doctor with --fix and --json
    Then the config file "rp.json" has the string "https://localhost:11131" at "/plate_solver/url"
    And the config file "rp.json" has "/ca_cert" pointing at the pki file "ca.pem"
    And the config file "rp.json" has the string "observatory" at "/plate_solver/auth/username"

  Scenario: --fix rewrites rp's guider scheme, CA trust, and credential once the target is provisioned
    Given a config file "phd2-guider.json" containing:
      """
      { "server": { "port": 11130, "auth": { "username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$YWJjZGVmZ2g$aGFuZHNldA" } } }
      """
    And a config file "rp.json" containing:
      """
      { "server": { "port": 11115 },
        "equipment": { "mount": { "alpaca_url": "http://localhost:11117",
                                   "guiding": { "url": "http://localhost:11130" } } } }
      """
    When I run doctor with --fix and --json
    Then the config file "rp.json" has the string "https://localhost:11130" at "/equipment/mount/guiding/url"
    And the config file "rp.json" has "/ca_cert" pointing at the pki file "ca.pem"
    And the config file "rp.json" has the string "observatory" at "/equipment/mount/guiding/auth/username"

  Scenario: A non-loopback client target is never joined
    Given a config file "rp.json" containing:
      """
      { "server": { "port": 11115, "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" } } }
      """
    And a config file "ui-htmx.json" containing:
      """
      { "server": { "port": 11120 }, "rp": { "base_url": "http://10.0.0.5:11115" } }
      """
    When I run doctor with --json
    Then the report has no checks named "joins.client-transport"

  Scenario: sentinel's Alpaca monitor scheme and credential are flagged and fixed by --fix
    Given a config file "ppba-driver.json" containing:
      """
      { "server": { "port": 11112 } }
      """
    And a config file "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "monitors": [ { "type": "alpaca_safety_monitor", "name": "PPBA",
                         "host": "localhost", "port": 11112, "scheme": "http" } ] }
      """
    When I run doctor with --fix and --json
    Then the config file "sentinel.json" has the string "https" at "/monitors/0/scheme"
    And the config file "sentinel.json" has the string "observatory" at "/monitors/0/auth/username"

  Scenario: sentinel's watchdog rp_url scheme is fixed, without duplicating auth.mismatch
    Given a config file "rp.json" containing:
      """
      { "server": { "port": 11115, "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" },
                    "auth": { "username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$YWJjZGVmZ2g$aGFuZHNldA" } } }
      """
    And a config file "sentinel.json" containing:
      """
      { "server": { "port": 11114 },
        "operation_watchdog": { "rp_url": "http://localhost:11115" } }
      """
    When I run doctor with --fix and --json
    Then the config file "sentinel.json" has the string "https://localhost:11115" at "/operation_watchdog/rp_url"
    And the report has no checks named "joins.client-auth"
