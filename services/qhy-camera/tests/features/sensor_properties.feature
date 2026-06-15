@serial
Feature: Sensor geometry and type
  Once connected, a camera reports its sensor geometry from the cached CCD
  info (G1): CameraXSize / CameraYSize in pixels and PixelSizeX / PixelSizeY
  in microns. SensorType is RGGB when the colour control is present and
  Monochrome otherwise, with BayerOffsetX / BayerOffsetY following the
  reported Bayer pattern (ST1). MaxADU is (2^OutputDataActualBits) - 1, i.e.
  65535 for a 16-bit sensor. The
  simulated QHY178M-Simulated camera is a 3072x2048 monochrome 16-bit sensor.

  Background:
    Given the qhy-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Sensor geometry reflects the simulated CCD info
    Then camera device 0 reports the sensor geometry:
      | property    | value |
      | CameraXSize | 3072  |
      | CameraYSize | 2048  |
    And camera device 0 reports a positive PixelSizeX
    And camera device 0 reports a positive PixelSizeY

  Scenario: A monochrome sensor reports SensorType Monochrome
    Then camera device 0 reports SensorType as Monochrome

  Scenario: A 16-bit sensor reports MaxADU 65535
    Then camera device 0 reports MaxADU as 65535

  Scenario: SensorName is reported and non-empty
    Then camera device 0 reports a non-empty SensorName
