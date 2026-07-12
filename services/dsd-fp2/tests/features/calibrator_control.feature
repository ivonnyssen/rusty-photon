Feature: Calibrator control
  The calibrator light responds to `calibrator_on(brightness)` and
  `calibrator_off()` by sending `[SLBR<NNNN>] + [SLON1]` and `[SLON0]`
  respectively. Brightness must be within the effective range
  `0..=max_brightness()` — the lower of the configured
  `cover_calibrator.max_brightness` and the 4096 hardware ceiling; values
  above are rejected with `INVALID_VALUE`. `max_brightness` reported
  through ASCOM defaults to `4096` unless configured lower. Below the
  configured `min_brightness` (default 250) the panel's EL output is
  non-linear, so a non-zero brightness under that floor is also rejected
  with `INVALID_VALUE`; `0` (the ASCOM "on at zero" state — the light
  stays logically on, at zero brightness) is always accepted regardless.

  Scenario: Calibrator on then off
    Given a connected FP2 device
    When calibrator_on is called with brightness 2048
    Then calibrator_state should eventually be Ready
    And brightness should be 2048
    When calibrator_off is called
    Then calibrator_state should eventually be Off

  Scenario: Calibrator rejects brightness above max
    Given a connected FP2 device
    When calibrator_on is called with brightness 4097 and the call is captured
    Then the call should fail with an invalid-value error

  Scenario: Calibrator rejects brightness below configured minimum
    Given a connected FP2 device
    When calibrator_on is called with brightness 100 and the call is captured
    Then the call should fail with an invalid-value error

  Scenario: Calibrator accepts zero even below configured minimum
    Given a connected FP2 device
    When calibrator_on is called with brightness 0
    Then brightness should be 0

  Scenario: Max brightness is 4096
    Given a running FP2 service
    Then max_brightness should be 4096

  Scenario: Calibrator commands are rejected when disconnected
    Given a running FP2 service
    When calibrator_on is called with brightness 2048 and the call is captured
    Then the call should fail with a not-connected error
    When calibrator_off is called and the call is captured
    Then the call should fail with a not-connected error
