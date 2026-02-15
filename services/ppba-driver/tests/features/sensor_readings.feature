Feature: Sensor Readings
  As an ASCOM client
  I want to read sensor values from the PPBA
  So that I can monitor environmental conditions and power stats

  Scenario: Voltage is in valid range
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 10 value should be approximately 12.5

  Scenario: Current is in valid range
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 11 value should be in range 0.0 to 20.0

  Scenario: Temperature is in valid range
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 12 value should be approximately 25.0

  Scenario: Humidity is in valid range
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 13 value should be in range 0.0 to 100.0

  Scenario: Average current from power stats
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 6 value should be non-negative

  Scenario: Amp hours from power stats
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 7 value should be non-negative

  Scenario: Watt hours from power stats
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 8 value should be non-negative

  Scenario: Uptime from power stats
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 9 value should be non-negative

  Scenario: DewA PWM precision is 128
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 2 value should be 128.0

  Scenario: DewB PWM precision is 64
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 3 value should be 64.0

  Scenario: Boolean switch values are 0.0 or 1.0
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 0 value should be 0.0 or 1.0

  Scenario: PWM switch values are in 0-255 range
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 2 value should be in range 0.0 to 255.0

  Scenario: Voltage sensor is positive
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 10 value should be positive
