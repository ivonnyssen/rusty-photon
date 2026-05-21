Feature: Calibrator control
  The calibrator light responds to `calibrator_on(brightness)` and
  `calibrator_off()` by sending `[SLBR<NNNN>] + [SLON1]` and `[SLON0]`
  respectively. Brightness must be between 0 and 4096 inclusive;
  values above are rejected with `INVALID_VALUE`. The maximum
  brightness reported through ASCOM is `4096`.

  Scenario: Calibrator on then off
    Given a connected FP2 device
    When calibrator_on is called with brightness 2048
    Then calibrator_state should be Ready
    And brightness should be 2048
    And the simulator brightness should be 2048
    And the simulator light should be on
    When calibrator_off is called
    Then calibrator_state should be Off
    And the simulator light should be off

  Scenario: Calibrator rejects brightness above max
    Given a connected FP2 device
    When calibrator_on is called with brightness 4097 and the call is captured
    Then the call should fail with an invalid-value error

  Scenario: Max brightness is 4096
    Given a freshly constructed FP2 device
    Then max_brightness should be 4096

  Scenario: Calibrator commands are rejected when disconnected
    Given a freshly constructed FP2 device
    When calibrator_on is called with brightness 100 and the call is captured
    Then the call should fail with a not-connected error
    When calibrator_off is called and the call is captured
    Then the call should fail with a not-connected error
