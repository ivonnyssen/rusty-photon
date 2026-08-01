@serial
Feature: Plate-solving polar alignment (end-to-end)
  The polar-align orchestrator measures the mount's RA-axis direction
  from three plate solves taken with only the RA axis moving, reports
  the axis-to-pole error as signed azimuth and altitude arcminutes on
  its /status endpoint, then holds an adjustment phase until the
  operator finishes it. rp deliberately ignores completion bodies, so
  the numeric contract is asserted on /status; session completion is
  asserted through rp's session API.

  These tests start OmniSim (telescope + camera), rp, and polar-align,
  with an in-process plate-solver stub whose canned solves are
  choreographed from a known injected axis error.

  Scenario: Measurement recovers a choreographed axis error
    Given a running Alpaca simulator
    And a stub plate solver choreographed for an axis error of 30.0 arcminutes east and -20.0 arcminutes in altitude
    And rp is running with a camera, a mount, the stub plate solver, and the polar-align orchestrator
    When a session is started via the REST API
    And the polar-align workflow reaches the "adjusting" phase
    And the adjustment is finished via the REST API
    Then the polar-align status should report an azimuth error within 2.0 arcminutes of 30.0
    And the polar-align status should report an altitude error within 2.0 arcminutes of -20.0
    And the stub plate solver should have received at least 3 solve requests
    And the session status should be "idle"

  Scenario: A failing solver aborts the measurement and frees the session
    Given a running Alpaca simulator
    And a stub plate solver that always fails
    And rp is running with a camera, a mount, the stub plate solver, and the polar-align orchestrator
    When a session is started via the REST API
    And the polar-align workflow reaches the "error" phase
    Then the session status should be "idle"

  Scenario: Finishing with no adjustment in progress is rejected
    Given the polar-align service is running standalone
    When the adjustment is finished via the REST API
    Then the finish request should be rejected with status 409

  Scenario: An invocation without required fields is rejected
    Given the polar-align service is running standalone
    When an invocation without a workflow id is posted via the REST API
    Then the invoke request should be rejected with status 400
    And the polar-align workflow phase should be "idle"
