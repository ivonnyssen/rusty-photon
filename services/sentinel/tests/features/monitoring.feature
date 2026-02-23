Feature: Safety monitor polling
  The sentinel service polls ASCOM Alpaca SafetyMonitor devices
  and reports their current state.

  Scenario: Monitor reports safe state
    Given a safety monitor that reports safe
    When the monitor is polled
    Then the state should be "Safe"

  Scenario: Monitor reports unsafe state
    Given a safety monitor that reports unsafe
    When the monitor is polled
    Then the state should be "Unsafe"

  Scenario: Monitor returns unknown on connection failure
    Given a safety monitor that is unreachable
    When the monitor is polled
    Then the state should be "Unknown"

  Scenario: Monitor returns unknown on ASCOM error
    Given a safety monitor that returns an ASCOM error
    When the monitor is polled
    Then the state should be "Unknown"

  Scenario: Monitor connects to device
    Given a safety monitor that accepts connections
    When the monitor connects
    Then the connection should succeed

  Scenario: Monitor disconnects from device
    Given a safety monitor that accepts connections
    When the monitor disconnects
    Then the disconnection should succeed
