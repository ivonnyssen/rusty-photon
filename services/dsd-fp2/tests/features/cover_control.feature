Feature: Cover control
  The cover responds to `open_cover` / `close_cover` by sending
  `[STRG<angle>] + [SMOV]`. ASCOM `CoverState` derives from the cached
  `[GMOV]` (motor) and `[GOPS]` (cover) values. `halt_cover` returns
  `MethodNotImplementedException` because the FP2 firmware exposes no
  abort opcode — which is what the ASCOM ICoverCalibratorV2 spec
  mandates "if cover movement cannot be interrupted".

  Scenario: Open cover succeeds and reports Open on the simulator
    Given a connected FP2 device
    When open_cover is called
    And the cache is refreshed by a single poll
    Then cover_state should be Open

  Scenario: Close cover succeeds and reports Closed on the simulator
    Given a connected FP2 device whose simulator starts open
    When close_cover is called
    And the cache is refreshed by a single poll
    Then cover_state should be Closed

  Scenario: Cover moves are rejected when disconnected
    Given a freshly constructed FP2 device
    When open_cover is called and the call is captured
    Then the call should fail with a not-connected error
    When close_cover is called and the call is captured
    Then the call should fail with a not-connected error
