Feature: Connection lifecycle
  The Deep Sky Dad FP2 driver opens its serial transport on the 0→1
  Connected transition, runs a `[GFRM]` handshake to verify the board
  identifies as `DeepSkyDad.FP2`, and tears the transport down on the
  1→0 Connected transition.

  Scenario: Device starts disconnected
    Given a freshly constructed FP2 device
    Then the device should report disconnected
    And cover_state should be Unknown
    And calibrator_state should be Unknown

  Scenario: Connect opens the transport and seeds cached state
    Given a freshly constructed FP2 device
    When the device is connected
    Then the device should report connected
    And the cached firmware board should be "DeepSkyDad.FP2"
    And cover_state should be Closed
    And calibrator_state should be Off

  Scenario: Disconnect closes the transport
    Given a freshly constructed FP2 device
    When the device is connected
    And the device is disconnected
    Then the device should report disconnected
    And cover_state should be Unknown
    And calibrator_state should be Unknown

  Scenario: Connect rejects firmware that is not DeepSkyDad.FP2
    Given a freshly constructed FP2 device whose simulator pretends to be DeepSkyDad.FP1
    When the device is connected and the connect attempt is captured
    Then the connect attempt should fail with a not-connected error
    And the device should report disconnected
