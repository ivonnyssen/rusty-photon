@serial
Feature: Camera enumeration and connection lifecycle
  svbony-camera enumerates connected SVBony cameras at startup and registers
  each as an ASCOM device, index 0, 1, 2, ..., on one port (C0). Unlike ZWO,
  each device's serial (`SVB_CAMERA_INFO.CameraSN`) arrives at enumeration
  time -- no camera needs to be opened first -- so the UniqueID is minted
  directly from enumeration (`SVBONY:{name}:{serial}`, falling back to
  `SVBONY:{name}:noserial-{index}` for a camera that reports an empty
  serial). Connecting a device (C1) opens it via the SDK; a `Connected`
  device that receives another `set_connected(true)` is a no-op (C1b). An
  open failure leaves the device not connected (C2). Disconnecting (C3)
  closes the device. Phase C/D scope: only the connection lifecycle itself
  is real here -- `SvbonyCamera`'s exposure/ROI/gain/cooling/sensor `Camera`
  methods are `NOT_IMPLEMENTED` stubs until Phase E
  (docs/plans/svbony-camera.md), so disconnect-cancels-an-in-flight-exposure
  (C3b) is `@wip`. With zero cameras discovered the service still starts,
  registering no Camera devices and logging a warning (C0b). Against the
  svbony-rs simulation backend exactly one camera is present
  (SV605CC-Simulated, 3008x3008, colour/OSC, 14-bit, cooled, trigger-capable,
  no ST4 port).

  Background:
    Given the svbony-camera service running with the simulation backend

  Scenario: The simulated camera is registered as device 0
    Then ASCOM camera device 0 is available
    And camera device 0 reports a non-empty UniqueID

  Scenario: A camera starts disconnected
    Then camera device 0 reports Connected as false

  Scenario: Connecting opens the camera
    When I connect camera device 0
    Then camera device 0 reports Connected as true

  Scenario: Reconnecting an already-connected camera is a no-op
    Given camera device 0 is connected
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
    Given the svbony-camera service running with an empty simulation backend
    Then no ASCOM camera devices are registered
    And the service is healthy
