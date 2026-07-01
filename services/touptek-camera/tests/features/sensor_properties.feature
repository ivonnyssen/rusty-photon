@serial
Feature: Sensor geometry, type, and signal
  Once connected, a camera reports its sensor geometry from the cached model
  info (G1): CameraXSize / CameraYSize in pixels and PixelSizeX / PixelSizeY
  in microns, with PixelSizeX equal to PixelSizeY for the simulated sensor.
  SensorType is RGGB when get_MonoMode reports colour and Monochrome
  otherwise, with BayerOffsetX / BayerOffsetY following get_RawFormat (ST1).
  Unlike zwo-camera, the ToupCam SDK exposes no native electrons-per-ADU
  field, so ElectronsPerADU is NOT_IMPLEMENTED (ST2). MaxADU is
  (2^BitDepth) - 1, i.e. 65535 for the 16-bit RAW path (ST3). The simulated
  ToupTek camera is a 3008x3008 colour 16-bit sensor (the ATR533C / IMX533) with
  3.76 micron square pixels; the floored full frame at every supported bin stays
  even, so CameraXSize and CameraYSize are reported as-is (no alignment-down needed).

  Background:
    Given the touptek-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Sensor geometry reflects the simulated camera info
    Then camera device 0 reports the sensor geometry:
      | property    | value |
      | CameraXSize | 3008  |
      | CameraYSize | 3008  |
    And camera device 0 reports a positive PixelSizeX
    And camera device 0 reports PixelSizeX equal to PixelSizeY

  Scenario: A colour sensor reports SensorType RGGB
    Then camera device 0 reports SensorType as RGGB

  Scenario: A 16-bit sensor reports MaxADU 65535
    Then camera device 0 reports MaxADU as 65535

  Scenario: ElectronsPerADU and FullWellCapacity are not implemented
    Then camera device 0 reports ElectronsPerADU as not implemented
    And camera device 0 reports FullWellCapacity as not implemented

  Scenario: SensorName is reported and non-empty
    Then camera device 0 reports a non-empty SensorName
