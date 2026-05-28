Feature: PulseGuide as rate-shifted tracking
  PulseGuide implements ASCOM autoguiding as a temporary rate shift on the
  targeted axis built from the standard tracking primitives — no `:P`
  command (that's the external ST4-jack rate setter, not a host-driven
  pulse). For each direction the call emits
  `:K<axis>` → `:G<axis>` (Tracking + ccw) → `:I<axis>` (shifted period) →
  `:J<axis>`, sets `IsPulseGuiding`, and spawns a watcher task that
  restores prior state after the requested duration.

  Direction → (axis, ccw, rate factor of sidereal):
  | Direction | Axis | ccw   | rate factor      |
  | East      | RA   | false | 1 - ra_fraction  |
  | West      | RA   | false | 1 + ra_fraction  |
  | North     | Dec  | false | dec_fraction     |
  | South     | Dec  | true  | dec_fraction     |

  Wire mode bytes (Tracking-Slow):
  | Direction | :G frame |
  | East/West | :G110    |
  | North     | :G210    |
  | South     | :G211    |

  Default `GuideRateRightAscension` / `GuideRateDeclination` is
  0.5 × sidereal (`SIDEREAL_DEG_PER_SEC ≈ 0.00417807`, so the default
  rate is approximately `0.00208904 deg/sec`).

  Scenario: CanPulseGuide is true when connected
    Given a running star-adventurer service
    When I connect the device
    Then CanPulseGuide should be true

  Scenario: CanSetGuideRates is true when connected
    Given a running star-adventurer service
    When I connect the device
    Then CanSetGuideRates should be true

  Scenario: IsPulseGuiding defaults to false after connect
    Given a running star-adventurer service
    When I connect the device
    Then IsPulseGuiding should be false

  Scenario: Default GuideRateRightAscension is half sidereal
    Given a running star-adventurer service
    When I connect the device
    Then GuideRateRightAscension should be approximately 0.00208904 within 0.00001

  Scenario: Default GuideRateDeclination is half sidereal
    Given a running star-adventurer service
    When I connect the device
    Then GuideRateDeclination should be approximately 0.00208904 within 0.00001

  Scenario: Setting GuideRateRightAscension within (0, sidereal) succeeds
    Given a running star-adventurer service
    When I connect the device
    And I set GuideRateRightAscension to 0.001
    Then GuideRateRightAscension should be approximately 0.001 within 0.00001

  Scenario: Setting GuideRateDeclination within (0, sidereal) succeeds
    Given a running star-adventurer service
    When I connect the device
    And I set GuideRateDeclination to 0.003
    Then GuideRateDeclination should be approximately 0.003 within 0.00001

  Scenario: Setting GuideRateRightAscension to zero fails
    Given a running star-adventurer service
    When I connect the device
    And I try to set GuideRateRightAscension to 0.0
    Then the operation should fail with invalid-value

  Scenario: Setting GuideRateRightAscension above sidereal fails
    # Upper bound is exclusive — fraction >= 1.0 would zero East's
    # rate factor and divide by zero in the step-period formula.
    # Match INDI's treatment of guide rate as a fraction strictly
    # less than 1. Using a value clearly above sidereal
    # (`SIDEREAL_DEG_PER_SEC ≈ 0.00417807`) so the rejection is
    # unambiguous even with floating-point comparison.
    Given a running star-adventurer service
    When I connect the device
    And I try to set GuideRateRightAscension to 0.01
    Then the operation should fail with invalid-value

  Scenario: Setting GuideRateDeclination to negative fails
    Given a running star-adventurer service
    When I connect the device
    And I try to set GuideRateDeclination to -0.001
    Then the operation should fail with invalid-value

  Scenario: PulseGuide North issues Tracking + CW commands on the Dec axis
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide North for 2000 ms
    Then IsPulseGuiding should be true
    And the mount should have received commands matching:
      | pattern |
      | :K2     |
      | :G210   |
      | :I2.*   |
      | :J2     |
    And IsPulseGuiding should become false within 5000 ms
    And the mount should have received command :K2

  Scenario: PulseGuide South issues Tracking + CCW commands on the Dec axis
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide South for 2000 ms
    Then IsPulseGuiding should be true
    And the mount should have received commands matching:
      | pattern |
      | :K2     |
      | :G211   |
      | :I2.*   |
      | :J2     |
    And IsPulseGuiding should become false within 5000 ms

  Scenario: PulseGuide East while tracking shifts the rate and restores sidereal
    # East slows tracking (period grows); after the pulse the watcher
    # re-issues sidereal tracking on RA so the user-observable
    # `Tracking` state is unchanged.
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide East for 2000 ms
    Then IsPulseGuiding should be true
    And IsPulseGuiding should become false within 5000 ms
    And the mount should have received commands matching:
      | pattern |
      | :K1     |
      | :G110   |
      | :I1.*   |
      | :J1     |
      | :K1     |
      | :G110   |
      | :I1.*   |
      | :J1     |

  Scenario: PulseGuide West while tracking shifts the rate and restores sidereal
    # West speeds tracking (period shrinks); same restore shape as East.
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide West for 2000 ms
    Then IsPulseGuiding should be true
    And IsPulseGuiding should become false within 5000 ms
    And the mount should have received commands matching:
      | pattern |
      | :K1     |
      | :G110   |
      | :I1.*   |
      | :J1     |
      | :K1     |
      | :G110   |
      | :I1.*   |
      | :J1     |

  Scenario: PulseGuide East while not tracking does not restore tracking
    # Without prior tracking, the watcher's RA restore branch is skipped
    # — only a final `:K1` (stop) is emitted. Tracking stays off.
    Given a running star-adventurer service
    When I connect the device
    And I pulse guide East for 200 ms
    Then IsPulseGuiding should become false within 2000 ms
    And the RA tracking-mode :G110 frame count should be exactly 1
    And Tracking should be false

  Scenario: PulseGuide fails while parked
    Given a running star-adventurer service
    And the device is parked
    When I try to pulse guide North for 100 ms
    Then the operation should fail with invalid-while-parked

  Scenario: PulseGuide fails while slewing
    # `the mount is slewing` seeds `running=true / goto=true` on both
    # mock axes, but the driver-side snapshot only reflects that after
    # the polling task does its first `:f` read post-connect. On a
    # fast runner the polling can lag the PulseGuide call, leaving
    # `slewing()` returning false and PulseGuide proceeding. Waiting
    # for `Slewing` to be visible before issuing the pulse closes
    # the race deterministically (the existing `Then Slewing should
    # be true` step polls with a 5 s deadline).
    Given a running star-adventurer service
    And the mount is slewing
    When I connect the device
    Then Slewing should be true
    When I try to pulse guide North for 100 ms
    Then the operation should fail with invalid-operation

  Scenario: PulseGuide fails while disconnected
    Given a running star-adventurer service
    When I try to pulse guide North for 100 ms
    Then the operation should fail with not-connected

  Scenario: A second pulse on the same axis is rejected while one is in flight
    # The first pulse is deliberately long (60s). The rejection under test
    # holds only while that pulse is in flight: pulse_guide sets
    # pulse_guiding_<axis> synchronously, then a detached watcher clears it
    # after `duration`, so a second same-axis pulse is refused only if it
    # reaches its in-flight check before the watcher fires. With the old
    # 1000ms pulse a slow / coverage-instrumented runner could let the
    # watcher clear the flag before the second pulse_guide arrived — the
    # second then succeeded ("no error captured") and the scenario flaked.
    # 60s dwarfs any plausible CI scheduling latency, so the rejection is
    # deterministic. The pulse never needs to finish: each scenario spawns
    # its own service (stopped at teardown, which aborts the detached
    # watcher), so the lingering pulse is harmless; pulse completion is
    # covered by the single- and perpendicular-pulse scenarios above.
    Given a running star-adventurer service
    When I connect the device
    And I pulse guide North for 60000 ms
    Then IsPulseGuiding should be true
    When I try to pulse guide South for 100 ms
    Then the operation should fail with invalid-operation

  Scenario: Perpendicular concurrent pulses (N on Dec, E on RA) both succeed
    # Durations are deliberately long (2 seconds each) to survive
    # slow-CI scheduling: the two `pulse_guide` calls plus the
    # following `IsPulseGuiding` read each take an HTTP round-trip,
    # so a short (<500ms) pulse can expire on a heavily-loaded
    # runner before the assertion fires. The 4s polling deadline
    # for completion gives both watchers room to wake.
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide North for 2000 ms
    And I pulse guide East for 2000 ms
    Then IsPulseGuiding should be true
    And IsPulseGuiding should become false within 4000 ms

  Scenario: set_tracking(false) during an RA pulse cancels the pulse restore
    # Cancellation rule: any axis-mutating call clears the pulse flag
    # before issuing its own wire commands, so the watcher's post-sleep
    # restore step bails out. The user-observable invariant is that
    # tracking stays off after the pulse — the watcher MUST NOT
    # re-issue tracking. The on-the-wire :G110 count after this
    # scenario is exactly 2: one from `enable tracking` (sidereal
    # start) and one from `pulse guide East` (rate-shifted start).
    # A third :G110 would indicate the watcher restored tracking
    # despite the cancellation.
    #
    # Pulse duration of 3 s gives the BDD client's HTTP round-trip
    # for `I disable tracking` plenty of headroom to clear the flag
    # before the watcher's sleep elapses on slow CI — a tight
    # duration here would race the watcher's restore decision.
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I pulse guide East for 3000 ms
    And I disable tracking
    Then IsPulseGuiding should become false within 5000 ms
    And Tracking should be false
    And the RA tracking-mode :G110 frame count should be exactly 2

  Scenario: AbortSlew during an in-flight pulse clears IsPulseGuiding
    Given a running star-adventurer service
    When I connect the device
    And I pulse guide North for 1000 ms
    And I abort the slew
    Then IsPulseGuiding should become false within 1000 ms

  Scenario: Duration zero succeeds with no wire activity
    # ASCOM permits zero-duration pulses; treat as a no-op.
    Given a running star-adventurer service
    When I connect the device
    And I pulse guide North for 0 ms
    Then IsPulseGuiding should be false
    And the Dec axis should have received no commands
