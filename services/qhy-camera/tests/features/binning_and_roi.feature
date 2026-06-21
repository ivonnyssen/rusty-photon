@serial
Feature: Binning and region-of-interest
  Binning is symmetric only: CanAsymmetricBin is false (B2) and MaxBinX /
  MaxBinY come from the SDK's valid binning modes. Setting a bin validates
  against those modes and rejects an unsupported value with INVALID_VALUE
  (B1); a bin change rescales the cached ROI by the bin ratio (B3). The ROI
  setters (StartX / StartY / NumX / NumY) accept any u32 (R1) — geometry is
  not validated at the setter but at StartExposure, which rejects a zero or
  out-of-bounds sub-frame with INVALID_VALUE (R2). The simulated QHY178M is
  3072x2048.

  Background:
    Given the qhy-camera service running with the simulation backend
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
      | 0     | 100   | 0       | 0       |
      | 100   | 0     | 0       | 0       |
      | 4000  | 100   | 0       | 0       |
      | 100   | 3000  | 0       | 0       |
      | 100   | 100   | 3000    | 0       |
      | 100   | 100   | 0       | 2000    |
