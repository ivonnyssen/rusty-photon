@wip @serial
Feature: Cooling
  Cooling is gated on the SDK Cooler control: CanSetCCDTemperature and
  CanGetCoolerPower are true only when the control is present, and the
  related getters return NOT_IMPLEMENTED otherwise (K1). When cooling is
  supported, CCDTemperature reads the current sensor temperature (K2),
  SetCCDTemperature validates the target against [-273.15, 80] and reads it
  back (K3), and CoolerOn / CoolerPower map to the SDK PWM controls (K4).
  The simulated QHY178M-Simulated camera has a cooler.

  Background:
    Given the qhy-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: A cooled camera advertises temperature control
    Then camera device 0 reports CanSetCCDTemperature as true
    And camera device 0 reports CanGetCoolerPower as true

  Scenario: The current CCD temperature is readable
    Then camera device 0 reports a finite CCDTemperature

  Scenario: A valid target temperature is accepted and read back
    When I set the target CCD temperature to -10.0 on camera device 0
    Then camera device 0 reports SetCCDTemperature as -10.0

  Scenario Outline: An out-of-range target temperature is rejected
    When I try to set the target CCD temperature to <target> on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE

    Examples:
      | target  |
      | -300.0  |
      | 100.0   |

  Scenario: Turning the cooler on is reflected in CoolerOn
    When I turn the cooler on for camera device 0
    Then camera device 0 reports CoolerOn as true
    And camera device 0 reports a CoolerPower between 0 and 100
