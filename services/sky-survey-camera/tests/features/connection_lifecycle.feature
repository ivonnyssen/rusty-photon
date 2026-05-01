@serial
Feature: Connection lifecycle
  set_connected validates that the cache directory is writable. The
  survey endpoint is probed with a short HEAD request; a probe failure
  is logged at warn! but does NOT block Connect (C3) — tying ASCOM
  Connect latency to a network round-trip would make the simulator
  flaky on slow links and in CI. A non-writable cache directory still
  fails Connect with ASCOM UNSPECIFIED_ERROR (C2). Disconnect cancels
  any in-flight exposure.

  Background:
    Given a sky-survey-camera with default optics

  Scenario: Successful connect when cache writable and SkyView reachable
    Given a writable cache directory
    And SkyView is reachable
    When I start the service
    And I connect the camera
    Then the camera is connected

  Scenario: Connect succeeds with a warn when SkyView is unreachable
    Given a writable cache directory
    And SkyView is unreachable
    When I start the service
    And I connect the camera
    Then the camera is connected

  Scenario: Connect fails when cache directory is not writable
    Given a non-writable cache directory
    And SkyView is reachable
    When I start the service
    And I try to connect the camera
    Then the connect attempt fails with ASCOM UNSPECIFIED_ERROR
    And the camera is not connected

  Scenario: Disconnect leaves the camera not connected
    Given a writable cache directory
    And SkyView is reachable
    When I start the service
    And I connect the camera
    And I disconnect the camera
    Then the camera is not connected
