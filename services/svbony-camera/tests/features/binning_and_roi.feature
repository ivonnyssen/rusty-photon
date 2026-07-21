@serial
@wip
Feature: Binning and region-of-interest
  Binning is symmetric only: CanAsymmetricBin is false (B2) and MaxBinX /
  MaxBinY come from the SDK's SupportedBins. Setting a bin validates against
  those modes and rejects an unsupported value with INVALID_VALUE (B1); a bin
  change rescales the cached ROI by the bin ratio (B3). The ROI setters
  (StartX / StartY / NumX / NumY) accept any u32 (R1) -- geometry is not
  validated at the setter but at StartExposure, which rejects a zero or
  out-of-bounds sub-frame with INVALID_VALUE (R2) and a sub-frame that
  violates SVBSetROIFormat's alignment rule -- width not a multiple of 8 or
  height not a multiple of 2, byte-for-byte the same rule zwo-camera enforces
  for ASI -- with INVALID_VALUE (R3). The simulated SV605CC-Simulated is
  3008x3008 with supported bins 1-4; whether CameraXSize/CameraYSize need
  the same "reported aligned down so every binned full frame is valid ROI"
  treatment as zwo-camera's R4 is a Phase E implementation decision (TBD),
  so this feature does not assert specific reduced values.

  Background:
    Given the svbony-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Asymmetric binning is not supported
    Then camera device 0 reports CanAsymmetricBin as false

  Scenario: A supported binning mode is accepted
    When I set BinX 2 and BinY 2 on camera device 0
    Then camera device 0 reports BinX as 2 and BinY as 2

  Scenario Outline: An unsupported binning value is rejected at the setter
    When I try to set BinX <bin_x> and BinY <bin_y> on camera device 0
    Then the set is rejected with ASCOM INVALID_VALUE

    Examples:
      | bin_x | bin_y |
      | 0     | 0     |
      | 99    | 99    |

  Scenario: The ROI setters accept any value
    When I set StartX 5000 NumX 5000 StartY 5000 NumY 5000 on camera device 0
    Then camera device 0 accepts the ROI without error

  Scenario Outline: An out-of-bounds sub-frame is rejected at StartExposure
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX <num_x> NumY <num_y> StartX <start_x> StartY <start_y> Duration 0.01 Light true
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | num_x | num_y | start_x | start_y |
      | 0     | 64    | 0       | 0       |
      | 64    | 0     | 0       | 0       |
      | 4000  | 64    | 0       | 0       |
      | 64    | 4000  | 0       | 0       |
      | 64    | 64    | 3008    | 0       |
      | 64    | 64    | 0       | 3008    |

  Scenario Outline: A misaligned sub-frame is rejected at StartExposure
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX <num_x> NumY <num_y> StartX 0 StartY 0 Duration 0.01 Light true
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | num_x | num_y |
      | 100   | 64    |
      | 64    | 47    |
