Feature: Sentinel builder and lifecycle
  The SentinelBuilder constructs the sentinel service from configuration,
  wiring monitors, notifiers, and the engine. It connects monitors during
  build and disconnects them on shutdown. Custom monitors and notifiers
  can be injected to bypass the config factories.

  Scenario: Builder succeeds with empty config
    Given an empty sentinel config
    When the sentinel is built
    Then the build should succeed

  Scenario: Builder connects monitor during build
    Given a sentinel config with monitor "Test" at localhost:11111 device 0
    When the sentinel is built
    Then the build should succeed
    And monitor "Test" should have been connected

  Scenario: Builder constructs correct Alpaca URL from config
    Given a sentinel config with monitor "Test" at myhost:9999 device 2
    When the sentinel is built
    Then monitor "Test" should have connected to "http://myhost:9999/api/v1/safetymonitor/2/connected"

  Scenario: Builder connects all configured monitors
    Given a sentinel config with monitor "Monitor1" at localhost:11111 device 0
    And a sentinel config with monitor "Monitor2" at localhost:11111 device 1
    When the sentinel is built
    Then monitor "Monitor1" should have been connected
    And monitor "Monitor2" should have been connected

  Scenario: Sentinel lifecycle completes with empty config
    Given an empty sentinel config
    And a pre-cancelled cancellation token
    When the sentinel is built and started
    Then the lifecycle should complete successfully

  Scenario: Sentinel disconnects monitors on shutdown
    Given a sentinel config with monitor "Test" at localhost:11111 device 0
    And a pre-cancelled cancellation token
    When the sentinel is built and started
    Then the lifecycle should complete successfully
    And monitor "Test" should have been disconnected

  Scenario: Builder uses injected monitors instead of config factory
    Given an empty sentinel config
    And an injected monitor named "injected"
    When the sentinel is built
    Then the build should succeed

  Scenario: Builder uses injected notifiers instead of config factory
    Given an empty sentinel config
    And an injected notifier of type "stub"
    When the sentinel is built
    Then the build should succeed
