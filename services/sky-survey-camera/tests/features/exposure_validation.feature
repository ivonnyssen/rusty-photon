@serial
Feature: StartExposure parameter validation
  StartExposure rejects out-of-range bin, sub-frame, and duration
  parameters with ASCOM INVALID_VALUE. Calling StartExposure while
  disconnected returns NOT_CONNECTED, and starting a new exposure while
  one is already in flight returns INVALID_OPERATION.

  Scenario: Disconnected camera rejects StartExposure
    Given the camera is started but not connected
    When I StartExposure with BinX 1 BinY 1 NumX 100 NumY 100 StartX 0 StartY 0 Duration 1.0
    Then the exposure is rejected with ASCOM NOT_CONNECTED

  Scenario Outline: Out-of-range bin values are rejected
    Given the camera is connected with the survey backend stubbed
    When I StartExposure with BinX <bin_x> BinY <bin_y> NumX 100 NumY 100 StartX 0 StartY 0 Duration 1.0
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | bin_x | bin_y |
      | 0     | 1     |
      | 1     | 0     |
      | 5     | 1     |
      | 1     | 5     |

  Scenario Outline: Sub-frames outside the binned sensor are rejected
    Given the camera is connected with the survey backend stubbed
    When I StartExposure with BinX 1 BinY 1 NumX <num_x> NumY <num_y> StartX <start_x> StartY <start_y> Duration 1.0
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | num_x | num_y | start_x | start_y |
      | 0     | 100   | 0       | 0       |
      | 100   | 0     | 0       | 0       |
      | 1000  | 100   | 0       | 0       |
      | 100   | 1000  | 0       | 0       |
      | 100   | 100   | 9999    | 0       |
      | 100   | 100   | 0       | 9999    |

  Scenario Outline: Out-of-range exposure duration is rejected
    Given the camera is connected with the survey backend stubbed
    When I StartExposure with BinX 1 BinY 1 NumX 100 NumY 100 StartX 0 StartY 0 Duration <duration>
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | duration |
      | -1.0     |
      | 100000.0 |

  Scenario: Second concurrent exposure is rejected
    Given the camera is connected with the survey backend stubbed
    And an exposure is already in flight
    When I StartExposure with BinX 1 BinY 1 NumX 100 NumY 100 StartX 0 StartY 0 Duration 1.0
    Then the exposure is rejected with ASCOM INVALID_OPERATION
