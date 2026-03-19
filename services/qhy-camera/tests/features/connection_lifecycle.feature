Feature: Connection lifecycle
  Camera and filter wheel devices manage connections to QHYCCD hardware
  through the SDK. Connection initializes the device and caches hardware
  capabilities. Disconnection releases the device.

  Scenario: Camera starts disconnected
    Given a camera device with mock SDK
    Then the camera should not be connected

  Scenario: Camera connects successfully
    Given a camera device with mock SDK
    When I connect the camera
    Then the camera should be connected

  Scenario: Camera disconnects successfully
    Given a camera device with mock SDK
    When I connect the camera
    And I disconnect the camera
    Then the camera should not be connected

  Scenario: Camera connect is idempotent
    Given a camera device with mock SDK
    When I connect the camera
    And I connect the camera
    Then the camera should be connected

  Scenario: Camera disconnect is idempotent
    Given a camera device with mock SDK
    When I disconnect the camera
    Then the camera should not be connected

  Scenario: Camera connect fails with SDK error
    Given a camera device with failing SDK
    When I try to connect the camera
    Then the operation should fail with a not-connected error

  Scenario: Filter wheel starts disconnected
    Given a filter wheel device with mock SDK
    Then the filter wheel should not be connected

  Scenario: Filter wheel connects successfully
    Given a filter wheel device with mock SDK
    When I connect the filter wheel
    Then the filter wheel should be connected

  Scenario: Filter wheel disconnects successfully
    Given a filter wheel device with mock SDK
    When I connect the filter wheel
    And I disconnect the filter wheel
    Then the filter wheel should not be connected

  Scenario: Filter wheel connect fails with SDK error
    Given a filter wheel device with failing SDK
    When I try to connect the filter wheel
    Then the operation should fail with a not-connected error
