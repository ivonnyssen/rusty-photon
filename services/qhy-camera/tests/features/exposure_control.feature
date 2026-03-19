Feature: Exposure control
  The camera supports single-frame exposures with async completion and abort.
  Exposure runs in a background task. The image is available after exposure
  completes. Dark frames are not supported.

  Scenario: Start exposure transitions to exposing state
    Given a connected camera device
    When I start a 1 second exposure
    Then camera_state should be exposing

  Scenario: Image not ready during exposure
    Given a connected camera device
    When I start a 1 second exposure
    Then image_ready should be false

  Scenario: Exposure completes and image becomes ready
    Given a connected camera device
    When I start a 0.01 second exposure
    And I wait for exposure to complete
    Then image_ready should be true
    And camera_state should be idle

  Scenario: Image array available after exposure
    Given a connected camera device
    When I start a 0.01 second exposure
    And I wait for exposure to complete
    Then image_array should be available

  Scenario: Abort exposure returns to idle
    Given a connected camera device
    When I start a 10 second exposure
    And I abort the exposure
    Then camera_state should be idle

  Scenario: Abort when idle is no-op
    Given a connected camera device
    When I abort the exposure
    Then camera_state should be idle

  Scenario: Dark frames are not supported
    Given a connected camera device
    When I try to start a dark frame exposure
    Then the operation should fail with an invalid-operation error

  Scenario: Cannot start exposure when already exposing
    Given a connected camera device
    When I start a 10 second exposure
    And I try to start another exposure
    Then the operation should fail with an invalid-operation error

  Scenario: Exposure fails when not connected
    Given a camera device with mock SDK
    When I try to start a 1 second exposure
    Then the operation should fail with a not-connected error

  Scenario: Last exposure duration recorded
    Given a connected camera device
    When I start a 0.01 second exposure
    And I wait for exposure to complete
    Then last_exposure_duration should be available

  Scenario: Last exposure start time recorded
    Given a connected camera device
    When I start a 0.01 second exposure
    And I wait for exposure to complete
    Then last_exposure_start_time should be available
