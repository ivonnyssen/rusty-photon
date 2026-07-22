@serial
Feature: Sensor geometry, type, and signal
  Once connected, a camera reports its sensor geometry from the cached
  SVB_CAMERA_PROPERTY (G1): CameraXSize / CameraYSize in pixels and
  PixelSizeX / PixelSizeY in microns (SVBGetSensorPixelSize), with
  PixelSizeX equal to PixelSizeY because SVBony exposes a single pixel
  size. SensorType is RGGB when the camera is a colour (OSC) model and
  Monochrome otherwise, derived at runtime from IsColorCam / BayerPattern --
  never hardcoded to the SV605CC's own pattern -- with BayerOffsetX /
  BayerOffsetY following the reported pattern (ST1, exact mapping a Phase E
  decision). Unlike zwo-camera, SVB_CAMERA_PROPERTY carries no native
  electrons-per-ADU field, so ElectronsPerADU is a NOT_IMPLEMENTED
  placeholder (ST2) rather than a native value -- confirm against real
  hardware whether the SDK exposes this some other way. MaxADU is
  (2^MaxBitDepth) - 1 (ST3). The simulated SV605CC-Simulated camera is a
  3008x3008 colour (Bayer RG) 14-bit sensor with a 3.76 micron pixel size.

  Background:
    Given the svbony-camera service running with the simulation backend
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

  Scenario: A 14-bit sensor reports MaxADU 16383
    Then camera device 0 reports MaxADU as 16383

  Scenario: SensorName is reported and non-empty
    Then camera device 0 reports a non-empty SensorName

  Scenario: ElectronsPerADU has no native SDK field and is not implemented
    Then camera device 0 reports ElectronsPerADU as NOT_IMPLEMENTED
