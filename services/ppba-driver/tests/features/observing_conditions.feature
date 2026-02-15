Feature: Observing Conditions
  As an ASCOM client
  I want to read environmental data from the PPBA
  So that I can monitor observing conditions

  Scenario: OC device static name from config
    Given an OC device with name "Test Weather Station"
    Then the OC device static name should be "Test Weather Station"

  Scenario: OC device unique ID from config
    Given an OC device with unique ID "custom-weather-001"
    Then the OC device unique ID should be "custom-weather-001"

  Scenario: OC device description contains Environmental
    Given an OC device with standard mock responses
    When I connect the OC device
    Then the OC device description should contain "Environmental"

  Scenario: OC device driver info contains PPBA and ObservingConditions
    Given an OC device with standard mock responses
    When I connect the OC device
    Then the OC device driver info should contain "PPBA"
    And the OC device driver info should contain "ObservingConditions"

  Scenario: OC device driver version is not empty
    Given an OC device with standard mock responses
    When I connect the OC device
    Then the OC device driver version should not be empty

  Scenario: Default average period is 5 minutes
    Given an OC device with standard mock responses
    When I connect the OC device
    Then the average period should be approximately 0.0833 hours

  Scenario: Set average period to 2 hours
    Given an OC device with standard mock responses
    When I connect the OC device
    And I set the average period to 2.0 hours
    Then the average period should be 2.0 hours

  Scenario: Set average period minimum 0
    Given an OC device with standard mock responses
    When I connect the OC device
    And I set the average period to 0.0 hours
    Then the average period should be 0.0 hours

  Scenario: Set average period maximum 24
    Given an OC device with standard mock responses
    When I connect the OC device
    And I set the average period to 24.0 hours
    Then the average period should be 24.0 hours

  Scenario: Set average period too small rejects
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to set the average period to -1.0 hours
    Then the last error code should be INVALID_VALUE

  Scenario: Set average period too large rejects
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to set the average period to 25.0 hours
    Then the last error code should be INVALID_VALUE

  Scenario: Set average period fractional 0.5 hours
    Given an OC device with standard mock responses
    When I connect the OC device
    And I set the average period to 0.5 hours
    Then the average period should be approximately 0.5 hours

  Scenario: Average period transitions from instantaneous to 1 hour and back
    Given an OC device with standard mock responses
    When I connect the OC device
    And I set the average period to 0.0 hours
    Then the average period should be 0.0 hours
    When I set the average period to 1.0 hours
    Then the average period should be 1.0 hours
    When I set the average period to 0.0 hours
    Then the average period should be 0.0 hours

  Scenario: Temperature reading with data
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then the temperature should be approximately 25.0

  Scenario: Temperature returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to read the temperature
    Then the last error code should be NOT_CONNECTED

  Scenario: Temperature returns VALUE_NOT_SET when samples age out
    Given an OC device with standard mock responses
    When I connect the OC device
    And I age out all samples
    Then reading the temperature should return VALUE_NOT_SET

  Scenario: Humidity reading with data
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then the humidity should be approximately 60.0

  Scenario: Humidity returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to read the humidity
    Then the last error code should be NOT_CONNECTED

  Scenario: Humidity returns VALUE_NOT_SET when samples age out
    Given an OC device with standard mock responses
    When I connect the OC device
    And I age out all samples
    Then reading the humidity should return VALUE_NOT_SET

  Scenario: Dewpoint reading with data
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then the dewpoint should be approximately 15.5

  Scenario: Dewpoint returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to read the dewpoint
    Then the last error code should be NOT_CONNECTED

  Scenario: Dewpoint returns VALUE_NOT_SET when samples age out
    Given an OC device with standard mock responses
    When I connect the OC device
    And I age out all samples
    Then reading the dewpoint should return VALUE_NOT_SET

  Scenario: Sensor readings reflect specific status values
    Given an OC device with custom status responses temp 18.3, humidity 45, dewpoint 8.7
    When I connect the OC device
    And I wait for status cache
    Then the temperature should be approximately 18.3
    And the humidity should be approximately 45.0
    And the dewpoint should be approximately 8.7

  Scenario: Sensor description for temperature
    Given an OC device with standard mock responses
    When I connect the OC device
    Then sensor description for "temperature" should contain "temperature"

  Scenario: Sensor description for humidity
    Given an OC device with standard mock responses
    When I connect the OC device
    Then sensor description for "humidity" should contain "humidity"

  Scenario: Sensor description for dewpoint
    Given an OC device with standard mock responses
    When I connect the OC device
    Then sensor description for "dewpoint" should contain "Dewpoint"

  Scenario: Sensor description is case insensitive
    Given an OC device with standard mock responses
    When I connect the OC device
    Then sensor description for "Temperature" and "TEMPERATURE" should match

  Scenario: Sensor description for unknown sensor returns NOT_IMPLEMENTED
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to get sensor description for "pressure"
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Sensor description for empty string returns INVALID_VALUE
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to get sensor description for ""
    Then the last error code should be INVALID_VALUE

  Scenario: Sensor description for truly unknown sensor returns INVALID_VALUE
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to get sensor description for "foobar"
    Then the last error code should be INVALID_VALUE

  Scenario: Sensor description returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to get sensor description for "temperature"
    Then the last error code should be NOT_CONNECTED

  Scenario: Time since last update with data
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then time since last update for "temperature" should be less than 1.0 seconds

  Scenario: Time since last update for humidity
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then time since last update for "humidity" should be less than 1.0 seconds

  Scenario: Time since last update for dewpoint
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then time since last update for "dewpoint" should be less than 1.0 seconds

  Scenario: Time since last update is case insensitive
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then time since last update for "Temperature" should be less than 1.0 seconds
    And time since last update for "TEMPERATURE" should be less than 1.0 seconds

  Scenario: Time since last update for empty string returns most recent
    Given an OC device with standard mock responses
    When I connect the OC device
    And I wait for status cache
    Then time since last update for "" should be less than 1.0 seconds

  Scenario: Time since last update for unknown sensor returns NOT_IMPLEMENTED
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to get time since last update for "pressure"
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Time since last update for truly unknown returns INVALID_VALUE
    Given an OC device with standard mock responses
    When I connect the OC device
    And I try to get time since last update for "foobar"
    Then the last error code should be INVALID_VALUE

  Scenario: Time since last update for unimplemented sensors returns NOT_IMPLEMENTED
    Given an OC device with standard mock responses
    When I connect the OC device
    Then time since last update should return NOT_IMPLEMENTED for all unimplemented sensors

  Scenario: Time since last update returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to get time since last update for "temperature"
    Then the last error code should be NOT_CONNECTED

  Scenario: Sensor description for unimplemented sensors returns NOT_IMPLEMENTED
    Given an OC device with standard mock responses
    When I connect the OC device
    Then sensor description should return NOT_IMPLEMENTED for all unimplemented sensors

  Scenario: Refresh returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to refresh the OC device
    Then the last error code should be NOT_CONNECTED

  Scenario: Refresh succeeds when connected
    Given an OC device with standard mock responses
    When I connect the OC device
    Then refreshing the OC device should succeed

  Scenario: Refresh updates sensor data
    Given an OC device with refresh update mock responses
    When I connect the OC device
    And I wait for status cache
    And I record the temperature
    And I refresh the OC device
    And I wait for status cache
    Then the temperature should have increased

  Scenario: Refresh with bad status returns INVALID_OPERATION
    Given an OC device with refresh bad status mock responses
    When I connect the OC device
    And I wait for status cache
    And I try to refresh the OC device
    Then the last error code should be INVALID_OPERATION

  Scenario: Cloud cover is not implemented
    Given an OC device with standard mock responses
    When I try to read cloud cover
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Pressure is not implemented
    Given an OC device with standard mock responses
    When I try to read pressure
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Rain rate is not implemented
    Given an OC device with standard mock responses
    When I try to read rain rate
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Sky brightness is not implemented
    Given an OC device with standard mock responses
    When I try to read sky brightness
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Sky quality is not implemented
    Given an OC device with standard mock responses
    When I try to read sky quality
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Sky temperature is not implemented
    Given an OC device with standard mock responses
    When I try to read sky temperature
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Star FWHM is not implemented
    Given an OC device with standard mock responses
    When I try to read star FWHM
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Wind direction is not implemented
    Given an OC device with standard mock responses
    When I try to read wind direction
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Wind gust is not implemented
    Given an OC device with standard mock responses
    When I try to read wind gust
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Wind speed is not implemented
    Given an OC device with standard mock responses
    When I try to read wind speed
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: OC connection fails with factory error maps to INVALID_OPERATION
    Given an OC device with a failing serial port "mock port not found"
    When I try to connect the OC device
    Then the last error code should be INVALID_OPERATION

  Scenario: OC connection fails with bad ping maps to INVALID_OPERATION
    Given an OC device with bad ping response
    When I try to connect the OC device
    Then the last error code should be INVALID_OPERATION

  Scenario: OC connection fails with bad status maps to INVALID_OPERATION
    Given an OC device with bad status response
    When I try to connect the OC device
    Then the last error code should be INVALID_OPERATION

  Scenario: Average period returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to read the average period
    Then the last error code should be NOT_CONNECTED

  Scenario: Set average period returns NOT_CONNECTED when disconnected
    Given an OC device with standard mock responses
    When I try to set the average period to 1.0 hours
    Then the last error code should be NOT_CONNECTED
