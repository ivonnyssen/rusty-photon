@serial @wip
Feature: Exposure lifecycle
  Discrete exposures are taken in trigger mode (OPTION_TRIGGER=1 plus
  Toupcam_Trigger(h,1)); the frame-ready callback drives the state machine
  through the callback-to-blocking bridge. StartExposure requires a connected
  device (E1, NOT_CONNECTED) and rejects a second exposure while one is in
  flight (E2, INVALID_OPERATION). A Duration outside [ExposureMin,
  ExposureMax] is rejected with INVALID_VALUE (E3). ToupTek sensors have no
  mechanical shutter, so a dark frame (Light = false) is accepted on every
  model and captured identically (E4) and HasShutter is false. A successful
  exposure triggers one frame, waits for the frame-ready event, pulls it, and
  produces an ImageArray of the binned sub-frame with ImageReady true and the
  last-exposure timestamps set (E5); CameraState is Exposing while in flight
  and PercentCompleted reaches 100 once ready (E6). AbortExposure cancels the
  trigger and discards the in-flight frame, and CanAbortExposure is true (E7).
  Trigger mode produces one whole frame with no partial readout, so there is
  no data-preserving graceful stop: CanStopExposure is false (E8, the ToupTek
  divergence from zwo-camera). A mid-exposure SDK error transitions to the
  Error state (E9, covered by unit tests against the mock SDK seam).

  Background:
    Given the touptek-camera service running with the simulation backend

  Scenario: A disconnected camera rejects StartExposure
    Given camera device 0 is not connected
    When I try to StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 64 StartX 0 StartY 0 Duration 0.01 Light true
    Then the exposure is rejected with ASCOM NOT_CONNECTED

  Scenario: A second exposure while one is in flight is rejected
    Given camera device 0 is connected
    And an exposure is in flight on camera device 0
    When I try to StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 64 StartX 0 StartY 0 Duration 0.01 Light true
    Then the exposure is rejected with ASCOM INVALID_OPERATION

  Scenario Outline: An out-of-range duration is rejected
    Given camera device 0 is connected
    When I try to StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 64 StartX 0 StartY 0 Duration <duration> Light true
    Then the exposure is rejected with ASCOM INVALID_VALUE

    Examples:
      | duration |
      | -1.0     |
      | 100000.0 |

  Scenario: A dark frame is accepted on a shutterless camera
    Given camera device 0 is connected
    And camera device 0 reports HasShutter as false
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 48 StartX 0 StartY 0 Duration 0.01 Light false
    And the exposure on camera device 0 completes
    Then camera device 0 reports ImageReady as true

  Scenario: A successful light exposure produces an image of the requested sub-frame
    Given camera device 0 is connected
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 48 StartX 0 StartY 0 Duration 0.01 Light true
    And the exposure on camera device 0 completes
    Then camera device 0 reports ImageReady as true
    And camera device 0 returns an ImageArray of 64 by 48
    And camera device 0 reports a set LastExposureStartTime
    And camera device 0 reports LastExposureDuration as 0.01

  Scenario: CameraState is Exposing during an in-flight capture
    Given camera device 0 is connected
    And an exposure is in flight on camera device 0
    Then camera device 0 reports CameraState as Exposing

  Scenario: PercentCompleted is 100 once the image is ready
    Given camera device 0 is connected
    When I StartExposure on camera device 0 with BinX 1 BinY 1 NumX 64 NumY 64 StartX 0 StartY 0 Duration 0.01 Light true
    And the exposure on camera device 0 completes
    Then camera device 0 reports CameraState as Idle
    And camera device 0 reports PercentCompleted as 100

  Scenario: Aborting an in-flight exposure leaves no image ready
    Given camera device 0 is connected
    And camera device 0 reports CanAbortExposure as true
    And an exposure is in flight on camera device 0
    When I abort the exposure on camera device 0
    Then camera device 0 reports ImageReady as false

  Scenario: A graceful stop is not supported in trigger mode
    Given camera device 0 is connected
    Then camera device 0 reports CanStopExposure as false
