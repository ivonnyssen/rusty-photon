Feature: Switch Metadata
  As an ASCOM client
  I want to query switch device properties
  So that I can understand the device capabilities

  Scenario: Device static name from config
    Given a switch device with name "Test PPBA"
    Then the switch device static name should be "Test PPBA"

  Scenario: Device unique ID from config
    Given a switch device with unique ID "custom-id-123"
    Then the switch device unique ID should be "custom-id-123"

  Scenario: Device description from config
    Given a switch device with description "Custom description"
    When I connect the switch device
    Then the switch device description should be "Custom description"

  Scenario: Device driver info contains PPBA
    Given a switch device with standard mock responses
    When I connect the switch device
    Then the switch device driver info should contain "PPBA"

  Scenario: Device driver version is not empty
    Given a switch device with standard mock responses
    When I connect the switch device
    Then the switch device driver version should not be empty

  Scenario: Max switch returns 16
    Given a switch device with standard mock responses
    Then the switch device max switch should be 16

  Scenario: All switches have names
    Given a switch device with standard mock responses
    When I connect the switch device
    Then all 16 switches should have non-empty names

  Scenario: All switches have descriptions
    Given a switch device with standard mock responses
    When I connect the switch device
    Then all 16 switches should have non-empty descriptions

  Scenario: Switch info consistency for all switches
    Given a switch device with standard mock responses
    When I connect the switch device
    Then all switches should have min less than max and positive step

  Scenario: Boolean switch min is 0 max is 1
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switch 0 min value should be 0.0
    And switch 0 max value should be 1.0

  Scenario: PWM switch min is 0 max is 255
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switch 2 min value should be 0.0
    And switch 2 max value should be 255.0

  Scenario: All switches have positive step
    Given a switch device with standard mock responses
    When I connect the switch device
    Then all 16 switches should have positive step values

  Scenario: Switch 15 is valid boundary
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switch 15 name should be queryable

  Scenario: Switch 16 is invalid boundary
    Given a switch device with standard mock responses
    When I connect the switch device
    Then querying switch 16 name should fail

  Scenario: set_switch_name is not implemented
    Given a switch device with standard mock responses
    When I try to set switch 0 name to "New Name"
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: Debug output includes config and uses non-exhaustive format
    Given a switch device with name "Debug Test PPBA"
    Then the switch device debug output should contain "PpbaSwitchDevice"
    And the switch device debug output should contain "Debug Test PPBA"
    And the switch device debug output should contain ".."

  Scenario: Device info methods return non-empty values
    Given a switch device with standard mock responses
    Then the switch device static name should not be empty
    And the switch device unique ID should not be empty
