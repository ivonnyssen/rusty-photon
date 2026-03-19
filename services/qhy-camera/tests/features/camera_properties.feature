Feature: Camera properties
  A connected camera exposes CCD information, binning modes, gain/offset
  ranges, exposure limits, cooler status, readout modes, and sensor info.
  All properties require a connected device.

  Scenario: CCD dimensions available after connect
    Given a connected camera device
    Then camera_x_size should be 4656
    And camera_y_size should be 3520

  Scenario: Pixel sizes available after connect
    Given a connected camera device
    Then pixel_size_x should be 3.76
    And pixel_size_y should be 3.76

  Scenario: Default binning is 1x1
    Given a connected camera device
    Then bin_x should be 1
    And bin_y should be 1

  Scenario: Max binning reflects available modes
    Given a connected camera device
    Then max_bin_x should be 4
    And max_bin_y should be 4

  Scenario: Binning can be changed
    Given a connected camera device
    When I set bin_x to 2
    Then bin_x should be 2

  Scenario: Invalid binning is rejected
    Given a connected camera device
    When I try to set bin_x to 5
    Then the operation should fail with an invalid-value error

  Scenario: ROI defaults to full effective area
    Given a connected camera device
    Then start_x should be 0
    And start_y should be 0
    And num_x should be 4656
    And num_y should be 3520

  Scenario: ROI can be modified
    Given a connected camera device
    When I set start_x to 100
    And I set num_x to 1000
    Then start_x should be 100
    And num_x should be 1000

  Scenario: Gain range available after connect
    Given a connected camera device
    Then gain_min should be 0
    And gain_max should be 100

  Scenario: Gain can be set within range
    Given a connected camera device
    When I set gain to 50
    Then gain should be 50

  Scenario: Gain out of range is rejected
    Given a connected camera device
    When I try to set gain to 150
    Then the operation should fail with an invalid-value error

  Scenario: Offset range available after connect
    Given a connected camera device
    Then offset_min should be 0
    And offset_max should be 200

  Scenario: Exposure limits available after connect
    Given a connected camera device
    Then exposure_min should be available
    And exposure_max should be available
    And exposure_resolution should be available

  Scenario: Camera state is idle when not exposing
    Given a connected camera device
    Then camera_state should be idle

  Scenario: Camera can abort exposure
    Given a connected camera device
    Then can_abort_exposure should be true
    And can_stop_exposure should be false

  Scenario: Camera has no shutter by default
    Given a connected camera device
    Then has_shutter should be false

  Scenario: Sensor is monochrome by default
    Given a connected camera device
    Then sensor_type should be monochrome

  Scenario: Sensor name parsed from unique ID
    Given a connected camera device
    Then sensor_name should be "QHY600M"

  Scenario: Readout modes available after connect
    Given a connected camera device
    Then readout_modes should have 2 entries

  Scenario: Fast readout supported
    Given a connected camera device
    Then can_fast_readout should be true

  Scenario: Cooler capabilities available
    Given a connected camera device
    Then can_set_ccd_temperature should be true
    And can_get_cooler_power should be true

  Scenario: Max ADU based on bit depth
    Given a connected camera device
    Then max_adu should be 65536

  Scenario: Properties fail when not connected
    Given a camera device with mock SDK
    When I try to read camera_x_size
    Then the operation should fail with a not-connected error

  Scenario: Driver info and version available
    Given a connected camera device
    Then driver_info should contain "QHY Camera"
    And driver_version should not be empty
