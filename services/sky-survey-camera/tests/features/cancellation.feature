@serial
Feature: Exposure cancellation
  AbortExposure and StopExposure cancel an in-flight survey fetch and
  leave ImageReady false. Calling either with no exposure in progress
  returns ASCOM INVALID_OPERATION.

  Scenario: AbortExposure cancels an in-flight exposure
    Given the camera is connected with the survey backend stubbed
    And an exposure is already in flight
    When I AbortExposure
    Then the cancellation succeeds
    And ImageReady is false

  Scenario: StopExposure cancels an in-flight exposure
    Given the camera is connected with the survey backend stubbed
    And an exposure is already in flight
    When I StopExposure
    Then the cancellation succeeds
    And ImageReady is false

  Scenario: AbortExposure with no exposure in progress is rejected
    Given the camera is connected with the survey backend stubbed
    When I AbortExposure
    Then the exposure is rejected with ASCOM INVALID_OPERATION

  Scenario: StopExposure with no exposure in progress is rejected
    Given the camera is connected with the survey backend stubbed
    When I StopExposure
    Then the exposure is rejected with ASCOM INVALID_OPERATION
