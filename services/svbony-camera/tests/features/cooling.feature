@serial
Feature: Cooling
  Cooling is gated on SVB_CAMERA_PROPERTY_EX.bSupportControlTemp:
  CanSetCCDTemperature and CanGetCoolerPower are true only when the camera
  supports temperature control, and the related getters return
  NOT_IMPLEMENTED otherwise (K1). When cooling is supported, CCDTemperature
  reads the current sensor temperature (SVB_CURRENT_TEMPERATURE, 0.1 degC
  units, K2), SetCCDTemperature validates the target against [-273.15, 80]
  and reads it back (SVB_TARGET_TEMPERATURE, also 0.1 degC units, K3), and
  CoolerOn / CoolerPower map to SVB_COOLER_ENABLE / SVB_COOLER_POWER (K4).
  Per the workspace's "no actuation on connect" tenet, connect / reconnect /
  config-apply MUST NOT touch SVB_COOLER_ENABLE or SVB_TARGET_TEMPERATURE --
  the cooler engages only on an explicit CoolerOn command (K5). The
  simulated SV605CC-Simulated camera supports temperature control.

  Background:
    Given the svbony-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: A temperature-controlled camera advertises cooling support
    Then camera device 0 reports CanSetCCDTemperature as true
    And camera device 0 reports CanGetCoolerPower as true

  Scenario: The current CCD temperature is readable
    Then camera device 0 reports a finite CCDTemperature

  Scenario: Connecting never enables the cooler
    Then camera device 0 reports CoolerOn as false

  Scenario: A valid target temperature is accepted and read back
    When I set the target CCD temperature to -10.0 on camera device 0
    Then camera device 0 reports SetCCDTemperature as -10.0

  Scenario Outline: An out-of-range target temperature is rejected
    When I try to set the target CCD temperature to <target> on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE

    Examples:
      | target |
      | -300.0 |
      | 100.0  |

  Scenario: Turning the cooler on is reflected in CoolerOn
    When I turn the cooler on for camera device 0
    Then camera device 0 reports CoolerOn as true
    And camera device 0 reports a CoolerPower between 0 and 100
