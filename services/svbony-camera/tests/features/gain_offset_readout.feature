@serial
@wip
Feature: Gain, offset, and readout modes
  Gain maps to SVB_GAIN and Offset maps to SVB_BLACK_LEVEL (SVBony's ASCOM
  Offset-equivalent control, read via SVBGetControlCaps / SVBGet-
  /SetControlValue). Both return the current SDK value, or NOT_IMPLEMENTED
  when the control is absent on the model (GO1). Setters validate against
  the cached [min, max] and reject an out-of-range value with INVALID_VALUE
  (GO2); GainMin / GainMax and OffsetMin / OffsetMax reflect the cached SDK
  limits (GO3). ReadoutModes is the driver's named list (exact mode names
  are a Phase E decision -- likely soft-trigger vs continuous video, see
  docs/services/svbony-camera.md); setting a mode validates the index,
  updates cached state, and rejects an unknown index with INVALID_VALUE
  (RM1).

  Background:
    Given the svbony-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Gain limits are ordered and the current gain is within them
    Then camera device 0 reports GainMin not greater than GainMax
    And camera device 0 reports a Gain within GainMin and GainMax

  Scenario: Setting gain to the maximum is accepted
    When I set Gain to GainMax on camera device 0
    Then camera device 0 reports Gain equal to GainMax

  Scenario: Setting gain above the maximum is rejected
    When I try to set Gain to one above GainMax on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE

  Scenario: Offset limits are ordered and the current offset is within them
    Then camera device 0 reports OffsetMin not greater than OffsetMax
    And camera device 0 reports an Offset within OffsetMin and OffsetMax

  Scenario: Setting offset below the minimum is rejected
    When I try to set Offset to one below OffsetMin on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE

  Scenario: The readout modes list is non-empty and the current mode is valid
    Then camera device 0 reports at least one ReadoutMode
    And camera device 0 reports a ReadoutMode index within the modes list

  Scenario: Selecting an out-of-range readout mode is rejected
    When I try to set ReadoutMode to 9999 on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE
