@serial
Feature: Sensor geometry, type, and signal
  Once connected, a camera reports its sensor geometry from the cached
  ASI_CAMERA_INFO (G1): CameraXSize / CameraYSize in pixels and PixelSizeX /
  PixelSizeY in microns, with PixelSizeX equal to PixelSizeY because ASI
  exposes a single pixel size. SensorType is RGGB when the camera is a colour
  model and Monochrome otherwise, with BayerOffsetX / BayerOffsetY following
  the reported Bayer pattern (ST1). ElectronsPerADU is a native value from
  ASI_CAMERA_INFO.ElecPerADU, not NOT_IMPLEMENTED (ST2). MaxADU is
  (2^BitDepth) - 1, i.e. 65535 for a 16-bit sensor (ST3). The simulated
  ASI2600MM-Pro-Simulated camera is a 6248x4176 monochrome 16-bit sensor.

  Background:
    Given the zwo-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Sensor geometry reflects the simulated camera info
    Then camera device 0 reports the sensor geometry:
      | property    | value |
      | CameraXSize | 6248  |
      | CameraYSize | 4176  |
    And camera device 0 reports a positive PixelSizeX
    And camera device 0 reports PixelSizeX equal to PixelSizeY

  Scenario: A monochrome sensor reports SensorType Monochrome
    Then camera device 0 reports SensorType as Monochrome

  Scenario: A 16-bit sensor reports MaxADU 65535
    Then camera device 0 reports MaxADU as 65535

  Scenario: ElectronsPerADU is a native positive value
    Then camera device 0 reports a positive ElectronsPerADU

  Scenario: SensorName is reported and non-empty
    Then camera device 0 reports a non-empty SensorName
