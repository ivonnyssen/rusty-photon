@serial
Feature: StartExposure survey-fetch path
  StartExposure derives a cutout from the optics and pointing, fetches
  it from the survey backend (or serves it from cache), and exposes it
  as ImageArray. Light=false skips the survey fetch and yields a zero
  frame. Backend errors propagate as ASCOM UNSPECIFIED_ERROR and do
  not poison the cache.

  Scenario: Healthy survey response yields an image of the requested dimensions
    Given the camera is connected with the survey backend stubbed
    And the survey backend returns a healthy FITS cutout
    When I StartExposure with default parameters
    Then the resulting image has dimensions 640 by 480

  Scenario: Light equals false yields a zero frame and no outbound request
    Given the camera is connected with the survey backend stubbed
    And the survey backend returns a healthy FITS cutout
    When I StartExposure with Light=false
    Then the resulting image has dimensions 640 by 480
    And every pixel of the resulting image is zero
    And no outbound survey HTTP request was made

  Scenario: Cache hit serves the image without an outbound request
    Given the camera is connected with the survey backend stubbed
    And the cache contains a hit for the next request
    When I StartExposure with default parameters
    Then the resulting image has dimensions 640 by 480
    And no outbound survey HTTP request was made

  Scenario: Survey HTTP 500 surfaces ASCOM UNSPECIFIED_ERROR
    Given the camera is connected with the survey backend stubbed
    And the survey backend returns HTTP 500
    When I StartExposure with default parameters
    Then the exposure fails with ASCOM UNSPECIFIED_ERROR

  Scenario: Survey timeout surfaces ASCOM UNSPECIFIED_ERROR
    Given the camera is connected with the survey backend stubbed
    And the survey backend exceeds the request timeout
    When I StartExposure with default parameters
    Then the exposure fails with ASCOM UNSPECIFIED_ERROR

  Scenario: Malformed FITS body surfaces ASCOM UNSPECIFIED_ERROR
    Given the camera is connected with the survey backend stubbed
    And the survey backend returns a malformed FITS body
    When I StartExposure with default parameters
    Then the exposure fails with ASCOM UNSPECIFIED_ERROR
