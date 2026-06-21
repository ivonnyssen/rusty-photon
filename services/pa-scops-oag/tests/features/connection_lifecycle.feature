Feature: Connection lifecycle
  The focuser device can be connected and disconnected
  via the ASCOM Alpaca HTTP API.

  Scenario: Device starts disconnected
    Given a running focuser service
    Then the device should be disconnected

  Scenario: Device connects successfully
    Given a running focuser service
    When I connect the device
    Then the device should be connected

  Scenario: Device disconnects after connect
    Given a running focuser service
    When I connect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Connecting an already-connected device is idempotent
    Given a running focuser service
    When I connect the device
    And I connect the device
    Then the device should be connected
