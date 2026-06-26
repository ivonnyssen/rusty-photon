@serial @wip
Feature: ST4 pulse guiding
  ToupTek exposes ST4 pulse-guiding natively (Toupcam_ST4PlusGuide), gated on
  the ST4 capability flag, so CanPulseGuide is true when the model reports ST4
  and the simulated camera reports it present (PG1). PulseGuide is
  asynchronous: it starts the ST4 pulse and returns immediately, with
  IsPulseGuiding reporting true until the pulse deadline (now + duration),
  which keeps it within ConformU's 1 s response target. PulseGuide on a
  disconnected device is rejected with NOT_CONNECTED (PG2). The no-ST4
  NOT_IMPLEMENTED branch and the asynchronous IsPulseGuiding timing are
  covered by unit tests, since the simulation backend always reports ST4
  present.

  Background:
    Given the touptek-camera service running with the simulation backend

  Scenario: Pulse guiding is supported via ST4
    Given camera device 0 is connected
    Then camera device 0 reports CanPulseGuide as true

  Scenario: A disconnected camera rejects PulseGuide
    Given camera device 0 is not connected
    When I try to PulseGuide on camera device 0 in direction North for 100 ms
    Then the PulseGuide is rejected with ASCOM NOT_CONNECTED
