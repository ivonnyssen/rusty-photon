@wip
Feature: Status Switch device
  A second ASCOM device on the same Alpaca server exposes two read-only
  switches the Rotator interface has no slot for: id 0 reports the Falcon's
  input voltage as a raw ADC count from VS (scale calibration deferred), id 1
  reports FA.limit_detect as a boolean.

  Scenario: MaxSwitch reports the two read-only switches
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I read MaxSwitch
    Then MaxSwitch should be 2

  Scenario: Neither switch is writable
    Given a running pa-falcon-rotator service
    When I connect the rotator
    Then CanWrite for id 0 should be false
    And CanWrite for id 1 should be false

  Scenario: GetSwitchValue for id 0 returns the raw VS ADC count
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device reports raw voltage 812
    And I read GetSwitchValue for id 0
    Then the switch value should be 812.0

  Scenario: GetSwitch for id 0 is true when the raw value is above zero
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device reports raw voltage 812
    And I read GetSwitch for id 0
    Then the switch boolean should be true

  Scenario: GetSwitch for id 1 mirrors the device's limit_detect flag
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device's limit_detect is true
    And I read GetSwitch for id 1
    Then the switch boolean should be true

  Scenario: GetSwitchValue for id 1 is 1.0 when limit_detect is set
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device's limit_detect is true
    And I read GetSwitchValue for id 1
    Then the switch value should be 1.0

  Scenario: SetSwitch on either switch is rejected with INVALID_OPERATION
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And I call SetSwitch on id 0 with true
    Then the set should fail with code 1035
