@serial
Feature: Camera cooling selects and holds a dark-library setpoint
  A camera's cooler_targets_c lists the temperatures the operator keeps dark
  libraries for: unique integers on the 5 °C grid from -40 to +15. When a
  session starts, rp commands the lowest listed rung and polls the cooler.
  Stabilizing within 1.0 °C of the rung with cooler power at or below 90 %
  adopts the rung for the whole session and emits cooler_stabilized. A
  trajectory that flattens above the rung, or one that holds the rung only
  at pegged power, marks tonight's floor: rp snaps up to the lowest rung at
  least 3 °C above the floor. When no rung qualifies, the cooler is switched
  off, cooler_unreachable is emitted, and the session proceeds uncooled.
  Session stop ramps the setpoint up in +5 °C steps before switching the
  cooler off. Every capture stamps cooler_setpoint_c and
  sensor_temperature_c on its exposure document. Cameras with an empty
  ladder are never touched.

  The simulator profile shipped by bdd-infra models ambient +10 °C with a
  maximum cooler delta of 40 °C: rungs above -30 stabilize with power
  headroom, while -30 itself only holds at 100 % power — tonight's floor.

  Background:
    Given a running Alpaca simulator
    And cooling is tuned for test speed
    And a test orchestrator that waits for a stop signal

  Scenario: The lowest reachable rung is adopted and announced
    Given a test webhook receiver subscribed to "cooler_stabilized"
    And rp is running with a camera with cooler targets "-10, 5" on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test webhook receiver should receive a "cooler_stabilized" event
    And the "cooler_stabilized" event payload field "target_c" should be the number -10
    And the camera cooler should be on

  Scenario: A rung holding only at pegged power snaps up to the next rung
    Given a test webhook receiver subscribed to "cooler_stabilized"
    And rp is running with a camera with cooler targets "-30, -10" on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test webhook receiver should receive a "cooler_stabilized" event
    And the "cooler_stabilized" event payload field "target_c" should be the number -10
    And the "cooler_stabilized" event payload should contain a "floor_c"

  Scenario: No reachable rung switches the cooler off and the session proceeds uncooled
    Given a test webhook receiver subscribed to "cooler_unreachable"
    And rp is running with a camera with cooler targets "-30" on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test webhook receiver should receive a "cooler_unreachable" event
    And the "cooler_unreachable" event payload field "warmest_target_c" should be the number -30
    And the camera cooler should be off
    And the session status should be "active"

  Scenario: Captured frames stamp the chosen setpoint and the sensor temperature
    Given a test webhook receiver subscribed to "cooler_stabilized"
    And rp is running with a camera with cooler targets "-10" on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test webhook receiver should receive a "cooler_stabilized" event
    When an MCP client connected to rp
    And the MCP client calls "capture" with camera "main-cam" for 100 ms
    And I fetch the document for the captured document_id
    Then the document field "cooler_setpoint_c" should be the number -10
    And the document should carry a numeric "sensor_temperature_c"

  Scenario: Session stop ramps the cooler warm and switches it off
    Given a test webhook receiver subscribed to "cooler_warmup_started" and "cooler_warmup_complete"
    And rp is running with a camera with cooler targets "5" on the simulator and the test orchestrator
    When a session is started via the REST API
    And the session is stopped via the REST API
    Then the test webhook receiver should receive a "cooler_warmup_started" event
    And the test webhook receiver should receive a "cooler_warmup_complete" event
    And the camera cooler should be off

  Scenario: A camera with an empty ladder is never cooled
    Given a test webhook receiver subscribed to "cooler_stabilized" and "cooler_unreachable"
    And rp is running with a camera with no cooler targets on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the session status should be "active"
    And the camera cooler should be off
    And the test webhook receiver should not have received any events
