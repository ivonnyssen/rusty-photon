@serial
Feature: Sensor geometry, type, and signal
  Once connected, a camera reports its sensor geometry from the cached
  ASI_CAMERA_INFO (G1): CameraXSize / CameraYSize in pixels and PixelSizeX /
  PixelSizeY in microns, with PixelSizeX equal to PixelSizeY because ASI
  exposes a single pixel size. SensorType is RGGB when the camera is a colour
  model and Monochrome otherwise, with BayerOffsetX / BayerOffsetY following
  the reported Bayer pattern (ST1). ElectronsPerADU is a native value from
  ASI_CAMERA_INFO.ElecPerADU, not NOT_IMPLEMENTED, and is read live because the
  SDK scales it by the gain register: the reported figure is the model's gain-0
  value divided by 10^(gain/200), ASI gain being in 0.1 dB units (ST2). MaxADU is
  (2^BitDepth) - 1, i.e. 65535 for a 16-bit sensor (ST3). The simulated
  ASI2600MM-Pro-Simulated camera is a 6248x4176 monochrome 16-bit sensor, but
  the reported CameraXSize is reduced to 6240 so the full frame divided by any
  supported bin remains a valid ASI ROI (width a multiple of 8); CameraYSize
  (4176) is already aligned.

  Background:
    Given the zwo-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Sensor geometry reflects the simulated camera info
    Then camera device 0 reports the sensor geometry:
      | property    | value |
      | CameraXSize | 6240  |
      | CameraYSize | 4176  |
    And camera device 0 reports a positive PixelSizeX
    And camera device 0 reports PixelSizeX equal to PixelSizeY

  Scenario: A monochrome sensor reports SensorType Monochrome
    Then camera device 0 reports SensorType as Monochrome

  Scenario: A 16-bit sensor reports MaxADU 65535
    Then camera device 0 reports MaxADU as 65535

  Scenario: ElectronsPerADU is a native positive value
    Then camera device 0 reports a positive ElectronsPerADU

  Scenario: ElectronsPerADU follows a change of gain
    The simulated camera is 0.25 e-/ADU at gain 0; 200 gain units is 20 dB,
    exactly a factor of ten, so the same camera reads 0.025 e-/ADU there.

    When I set Gain to 0 on camera device 0
    Then camera device 0 reports ElectronsPerADU as 0.25
    When I set Gain to 200 on camera device 0
    Then camera device 0 reports ElectronsPerADU as 0.025

  Scenario: SensorName is reported and non-empty
    Then camera device 0 reports a non-empty SensorName
