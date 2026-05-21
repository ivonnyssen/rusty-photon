Feature: Connection lifecycle
  The dsd-fp2 binary opens its serial transport on the 0→1 Connected
  transition, runs a `[GFRM]` handshake to verify the board identifies
  as `DeepSkyDad.FP2`, and tears the transport down on the 1→0
  Connected transition.

  Scenario: Device starts disconnected
    Given a running FP2 service
    Then the device should report disconnected
    And cover_state should be Unknown
    And calibrator_state should be Unknown

  Scenario: Connect opens the transport and seeds cached state
    Given a running FP2 service
    When the device is connected
    Then the device should report connected
    And cover_state should eventually be Closed
    And calibrator_state should eventually be Off

  Scenario: Disconnect closes the transport
    Given a running FP2 service
    When the device is connected
    And the device is disconnected
    Then the device should report disconnected
    And cover_state should be Unknown
    And calibrator_state should be Unknown
