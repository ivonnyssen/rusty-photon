@serial
Feature: Binning and region-of-interest
  Binning is digital (OPTION_BINNING, sum or average) and symmetric only:
  CanAsymmetricBin is false (B2) and MaxBinX / MaxBinY come from the supported
  factors (4 for the simulated camera). It is reported to ASCOM as binning but
  is not advertised as hardware binning. Setting a bin validates against the
  supported factors and rejects an unsupported value with INVALID_VALUE (B1);
  a bin change rescales the cached ROI by the bin ratio (B3). The ROI setters
  (StartX / StartY / NumX / NumY) accept any u32 (R1) -- geometry is not
  validated at the setter but at StartExposure, which rejects a zero or
  out-of-bounds sub-frame with INVALID_VALUE (R2) and a sub-frame violating
  the ToupTek alignment rules -- any of StartX, StartY, NumX, NumY odd, or
  NumX / NumY below 8 -- with INVALID_VALUE (R3). The simulated camera is
  6248x4176, and the floored full frame at every supported bin
  (floor(CameraXSize / bin) by floor(CameraYSize / bin)) is even and in-bounds,
  so it stays a valid ROI (R4) -- including bin 3, where 6248 / 3 floors to 2082.

  Background:
    Given the touptek-camera service running with the simulation backend
    And camera device 0 is connected

  Scenario: Asymmetric binning is not supported and the max bin is 4
    Then camera device 0 reports CanAsymmetricBin as false
    And camera device 0 reports MaxBinX as 4 and MaxBinY as 4

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
    When I set StartX 9000 NumX 9000 StartY 9000 NumY 9000 on camera device 0
    Then camera device 0 accepts the ROI without error

  Scenario Outline: An out-of-bounds sub-frame is rejected at StartExposure
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX <num_x> NumY <num_y> StartX <start_x> StartY <start_y> Duration 0.01 Light true
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | num_x | num_y | start_x | start_y |
      | 0     | 64    | 0       | 0       |
      | 64    | 0     | 0       | 0       |
      | 8000  | 64    | 0       | 0       |
      | 64    | 6000  | 0       | 0       |
      | 64    | 64    | 6248    | 0       |
      | 64    | 64    | 0       | 4176    |

  Scenario Outline: A misaligned sub-frame is rejected at StartExposure
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX <num_x> NumY <num_y> StartX <start_x> StartY <start_y> Duration 0.01 Light true
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | num_x | num_y | start_x | start_y |
      | 65    | 64    | 0       | 0       |
      | 64    | 47    | 0       | 0       |
      | 4     | 64    | 0       | 0       |
      | 64    | 64    | 9       | 0       |

  # R4: the floored full frame at every supported bin
  # (NumX = floor(CameraXSize/bin), NumY = floor(CameraYSize/bin)) is even,
  # in-bounds, and must expose. Bin 3 is the case that floors: 6248/3 -> 2082.
  Scenario Outline: A binned full frame is accepted at every bin
    When I StartExposure on camera device 0 with BinX <bin> BinY <bin> NumX <num_x> NumY <num_y> StartX 0 StartY 0 Duration 0.01 Light true
    Then the exposure on camera device 0 completes

    Examples:
      | bin | num_x | num_y |
      | 2   | 3124  | 2088  |
      | 3   | 2082  | 1392  |
      | 4   | 1562  | 1044  |
