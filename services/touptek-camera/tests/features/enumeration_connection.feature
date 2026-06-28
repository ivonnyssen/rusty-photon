@serial
Feature: Camera enumeration and connection lifecycle
  touptek-camera enumerates every connected ToupTek camera at startup and
  registers each as an ASCOM device, index 0, 1, 2, ..., on one port (C0).
  Each device's UniqueID is derived from its SDK device id; because
  Toupcam_EnumV2 returns the stable id directly, enumeration needs no
  per-camera open (a simplification over ZWO, which must open the camera to
  read its serial), so two identical-model cameras are distinguished by id.
  Connect is per-device (C4): connecting or disconnecting one camera does not
  affect the others. Opening a device (C1) selects RAW16 plus trigger mode and
  caches the model capability flags, geometry, and exposure / gain / offset
  ranges. An open failure leaves the device not connected (C2). Disconnect
  closes the device (Toupcam_Stop + Toupcam_Close) and cancels any in-flight
  exposure (C3). With zero cameras discovered the service still starts,
  registering no Camera devices and logging a warning. Against the touptek-rs
  simulation backend exactly one camera is present: a Simulated ToupTek Camera
  (id sim-0, 6248x4176, monochrome, 16-bit). ToupTek ships no filter wheel or
  focuser in this SDK, so the service registers Camera devices only.

  Background:
    Given the touptek-camera service running with the simulation backend

  Scenario: The simulated camera is registered as device 0
    Then ASCOM camera device 0 is available
    And camera device 0 reports a non-empty UniqueID

  Scenario: A camera starts disconnected
    Then camera device 0 reports Connected as false

  Scenario: Connecting opens the camera
    When I connect camera device 0
    Then camera device 0 reports Connected as true

  Scenario: Disconnecting leaves the camera not connected
    When I connect camera device 0
    And I disconnect camera device 0
    Then camera device 0 reports Connected as false

  Scenario: Disconnecting cancels an in-flight exposure
    Given camera device 0 is connected
    And an exposure is in flight on camera device 0
    When I disconnect camera device 0
    Then camera device 0 reports ImageReady as false
    And camera device 0 reports Connected as false

  Scenario: The service starts with no Camera devices when no camera is present
    Given the touptek-camera service running with an empty simulation backend
    Then no ASCOM camera devices are registered
    And the service is healthy
