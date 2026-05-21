Feature: Cover control
  The cover responds to `open_cover` / `close_cover` by sending
  `[STRG<angle>] + [SMOV]`. ASCOM `CoverState` derives from the cached
  `[GMOV]` (motor) and `[GOPS]` (cover) values that the while-open poll
  task refreshes every `polling_interval`.

  Scenario: Open cover succeeds and reports Open on the simulator
    Given a connected FP2 device
    When open_cover is called
    Then cover_state should eventually be Open

  Scenario: Close cover from open returns to Closed
    Given a connected FP2 device
    When the cover has been opened
    And close_cover is called
    Then cover_state should eventually be Closed

  Scenario: Cover moves are rejected when disconnected
    Given a running FP2 service
    When open_cover is called and the call is captured
    Then the call should fail with a not-connected error
    When close_cover is called and the call is captured
    Then the call should fail with a not-connected error
