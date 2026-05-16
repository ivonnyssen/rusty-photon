@wip
Feature: Connection lifecycle
  The pa-falcon-rotator service exposes a Rotator and a Status Switch device on
  a single ASCOM Alpaca server. A successful first connect performs the Falcon
  handshake (F# → FV → DR:0 → FA → VS) before any device-bound commands. The
  port is shared by both devices; the second device's connect is a no-op
  refcount bump and the port closes only when the last device disconnects.

  Scenario: Rotator starts disconnected
    Given a running pa-falcon-rotator service
    Then the rotator should be disconnected

  Scenario: Rotator connects successfully and runs the handshake
    Given a running pa-falcon-rotator service
    When I connect the rotator
    Then the rotator should be connected
    And the handshake should have issued F# before any other command

  Scenario: Rotator disconnects after connecting
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I disconnect the rotator
    Then the rotator should be disconnected

  Scenario: Connect is idempotent
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I connect the rotator
    Then the rotator should be connected

  Scenario: Shared port stays open while either device is connected
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I connect the status switch
    And I disconnect the rotator
    Then the status switch should be connected
    And the handshake should have run exactly once

  Scenario: Shared port closes only when the last device disconnects
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I connect the status switch
    And I disconnect the rotator
    And I disconnect the status switch
    Then the rotator should be disconnected
    And the status switch should be disconnected

  # Error-model contract: every device-bound op rejects with NOT_CONNECTED
  # (ASCOM code 1031 = 0x407) when called while disconnected. Three
  # representative ops — one rotator read, one rotator write, one switch
  # read — pin the guard without re-listing every property.

  Scenario: Rotator Position read returns NOT_CONNECTED when disconnected
    Given a running pa-falcon-rotator service
    When I read Position
    Then the operation should fail with code 1031

  Scenario: Rotator MoveAbsolute returns NOT_CONNECTED when disconnected
    Given a running pa-falcon-rotator service
    When I call MoveAbsolute with 90.00
    Then the move should fail with code 1031

  Scenario: Status Switch GetSwitchValue returns NOT_CONNECTED when disconnected
    Given a running pa-falcon-rotator service
    When I read GetSwitchValue for id 0
    Then the switch read should fail with code 1031
