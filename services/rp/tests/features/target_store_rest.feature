Feature: Target REST endpoints (P1)
  Mirrors the target MCP tools (target_store_crud.feature) over plain
  REST, body-for-body, so a UI and an MCP client see the same shapes
  (rp.md § REST Endpoints → Targets, Decision 10's minimal operator
  surface — P3-imported targets must never be stranded with no UI).

    GET    /api/targets                list targets with derived progress
    GET    /api/targets/{slug}         fetch one target with derived progress
    POST   /api/targets                create/upsert (as add_target)
    PUT    /api/targets/{slug}         edit in place (as update_target)
    PUT    /api/targets/{slug}/goals   replace the goal set (as set_goals)
    DELETE /api/targets/{slug}         remove the plan row (as delete_target)

  Scenario: POST /api/targets creates a target
    Given rp is running with a target store
    When I POST /api/targets with display_name "REST Frame" ra_hours 5.0 dec_degrees 10.0
    Then the targets API response status should be 200
    And the targets API response should carry slug "rest-frame"

  Scenario: GET /api/targets lists created targets
    Given rp is running with a target store
    And a target named "REST Frame" has been created via POST /api/targets
    When I GET /api/targets
    Then the targets API response status should be 200
    And the targets API target list should contain exactly "rest-frame"

  Scenario: GET /api/targets/{slug} fetches one target
    Given rp is running with a target store
    And a target named "REST Frame" has been created via POST /api/targets
    When I GET the target at slug "rest-frame"
    Then the targets API response status should be 200
    And the targets API response should carry slug "rest-frame"

  Scenario: PUT /api/targets/{slug} edits a target in place
    Given rp is running with a target store
    And a target named "REST Frame" has been created via POST /api/targets
    When I PUT the target at slug "rest-frame" setting display_name to "REST Frame (edited)"
    Then the targets API response status should be 200
    And the targets API response should carry display_name "REST Frame (edited)"

  Scenario: PUT /api/targets/{slug}/goals replaces the goal set
    Given rp is running with a target store
    And a target named "REST Frame" has been created via POST /api/targets
    When I PUT goals for the target at slug "rest-frame":
      | filter | binning | exposure | desired_count |
      | L      | 1x1     | 300s     | 20            |
    Then the targets API response status should be 200
    And the targets API response should carry exactly these goals:
      | filter | binning | exposure | desired_count |
      | L      | 1x1     | 300s     | 20            |

  Scenario: DELETE /api/targets/{slug} removes the plan row
    Given rp is running with a target store
    And a target named "REST Frame" has been created via POST /api/targets
    When I DELETE the target at slug "rest-frame"
    Then the targets API response status should be 200
    And GET /api/targets should list no targets
