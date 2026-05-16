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
