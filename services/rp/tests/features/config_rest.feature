Feature: Plain-REST configuration endpoints
  rp exposes its own configuration over plain REST — no Alpaca envelope,
  because rp is not an ASCOM device — reusing the cross-driver config
  protocol types from rusty-photon-config:
    GET /api/config        returns {"config": <effective config, secrets redacted>, "overrides": []}
    GET /api/config/schema returns {"schema": <JSON Schema>, "locked_fields": [], "read_only_fields": ["server.port"]}
    PUT /api/config        accepts a full config JSON and persists it to the config file
  rp has no in-process reload, so every changed field is classified
  "restart_required" and the apply status stays "ok" — the persisted file
  takes effect on the next rp start. Validation failures are HTTP 200 with
  status "invalid" and errors[], leaving the file untouched; a malformed
  JSON body is HTTP 400; a body over axum's default 2 MiB request limit is
  HTTP 413. Secrets are redacted to the sentinel "********";
  submitting the sentinel back means "keep the stored secret unchanged".
  These endpoints spawn rp with a temp config only — no simulator needed.

  Scenario: GET /api/config redacts a stored device password
    Given a temp rp config with a camera whose stored auth password is "hunter2"
    And rp is started with that config file
    When I GET /api/config
    Then the config response status should be 200
    And the fetched config field "/equipment/cameras/0/auth/password" should be "********"
    And the config overrides list should be empty

  Scenario: GET /api/config/schema lists server.port as the only read-only field
    Given a temp rp config with no equipment
    And rp is started with that config file
    When I GET /api/config/schema
    Then the config response status should be 200
    And the schema read-only fields should be exactly "server.port"
    And the schema locked fields should be empty

  Scenario: PUT /api/config persists a changed field as restart_required with status ok
    Given a temp rp config with no equipment
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/imaging/cache_max_mib" to "256"
    Then the config response status should be 200
    And the apply status should be "ok"
    And the restart-required list should be exactly "imaging.cache_max_mib"
    And the reload list should be empty
    And the config file JSON at "/imaging/cache_max_mib" should be the number 256

  Scenario: PUT /api/config with an unchanged config reports ok and nothing to restart
    Given a temp rp config with no equipment
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config unchanged
    Then the config response status should be 200
    And the apply status should be "ok"
    And the restart-required list should be empty
    And the reload list should be empty

  Scenario: PUT /api/config with an out-of-range site latitude is rejected and the file is untouched
    Given a temp rp config with no equipment
    And rp is started with that config file
    And I remember the config file bytes
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting the site latitude to 91.0
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "site.latitude_degrees"
    And the config file bytes should be unchanged

  Scenario: PUT /api/config with a malformed body returns 400
    Given a temp rp config with no equipment
    And rp is started with that config file
    And I remember the config file bytes
    When I PUT /api/config with body "this is not json"
    Then the config response status should be 400
    And the config file bytes should be unchanged

  Scenario: PUT /api/config with an oversized body is rejected before parsing
    Given a temp rp config with no equipment
    And rp is started with that config file
    And I remember the config file bytes
    When I PUT /api/config with a body just over the 2 MiB request limit
    Then the config response status should be 413
    And the config file bytes should be unchanged

  Scenario: A redacted device password round-trips PUT unchanged on disk
    Given a temp rp config with a camera whose stored auth password is "hunter2"
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config unchanged
    Then the config response status should be 200
    And the apply status should be "ok"
    And the config file JSON at "/equipment/cameras/0/auth/password" should be "hunter2"
