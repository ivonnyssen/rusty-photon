@serial
Feature: Rotator MCP tools
  move_rotator moves a rotator to an absolute sky angle in degrees
  (0.0 inclusive to 360.0 exclusive — the ASCOM Position frame, which
  honors any sync offset the driver holds), blocks polling IsMoving
  until idle, and reads back the sky and mechanical angles;
  get_rotator_position reads them without moving. Both tools address
  the device as rotator_id or train_id — exactly one — where train_id
  resolves through the optical-train model and requires the train to
  contain exactly one rotator. The angle is validated before any
  motion. move_rotator reports moved_trains, every train containing
  the rotator. When the moved trains include the guiding train and
  the guider reports an active guide loop, the move runs the
  rotate-while-guiding ladder: pause corrections (output-only), move,
  decide the calibration (kept when PHD2 has a connected rotator or
  the sky-angle change is within recalibrate_above_deg, cleared
  otherwise), re-select the guide star, resume. The result's
  guiding_ladder field records the outcome and is null whenever the
  ladder did not engage — including rotators outside the guiding
  train and moves while guiding is idle, which run bare. The move
  emits a move_rotator_started / move_rotator_complete /
  move_rotator_failed operation triple carrying no predictive
  deadline.

  Scenario: Tool catalog includes the rotator tools
    Given a running Alpaca simulator
    And rp is running with a rotator on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "move_rotator"
    And the tool list should include "get_rotator_position"

  Scenario: get_rotator_position reads the angle and motion state
    Given a running Alpaca simulator
    And rp is running with a rotator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_rotator_position" with rotator "main-rotator"
    Then the rotator result field "rotator_id" should be "main-rotator"
    And the rotator result should carry a numeric "angle"
    And the rotator result should carry a numeric "mechanical_angle"
    And the rotator result field "is_moving" should be false

  Scenario: move_rotator moves to an absolute sky angle and reads back
    Given a running Alpaca simulator
    And rp is running with a rotator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 5.0
    Then the rotator result field "rotator_id" should be "main-rotator"
    And the rotator result field "angle" should be 5.0 within 0.1
    And the rotator result should list no moved trains

  Scenario: move_rotator via train addressing reports the moved trains
    Given a running Alpaca simulator
    And rp is running with a rotator on the simulator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with train "main" to angle 5.0
    Then the rotator result field "rotator_id" should be "main-rotator"
    And the rotator result should list moved train "main"

  Scenario: get_rotator_position accepts train addressing
    Given a running Alpaca simulator
    And rp is running with a rotator on the simulator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "get_rotator_position" with train "main"
    Then the rotator result field "rotator_id" should be "main-rotator"
    And the rotator result field "is_moving" should be false

  Scenario: move_rotator emits the operation triple
    Given a running Alpaca simulator
    And a test webhook receiver subscribed to "move_rotator_started" and "move_rotator_complete"
    And rp is running with a rotator on the simulator
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 5.0
    Then the test webhook receiver should receive a "move_rotator_started" event
    And the test webhook receiver should receive a "move_rotator_complete" event
    And the "move_rotator_started" event payload field "rotator_id" should be "main-rotator"
    And the "move_rotator_started" event payload should contain a "angle"
    And the "move_rotator_complete" event payload should contain a "moved_trains"

  Scenario Outline: move_rotator rejects an out-of-range angle before any motion
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle <angle>
    Then the tool call should return an error
    And the error message should contain "angle"

    Examples:
      | angle |
      | 360.0 |
      | -5.0  |

  Scenario: move_rotator rejects rotator_id combined with train_id
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" and train "main" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "exactly one of rotator_id or train_id"

  Scenario: move_rotator rejects a call with no addressing
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with no addressing to angle 5.0
    Then the tool call should return an error
    And the error message should contain "exactly one of rotator_id or train_id"

  Scenario: move_rotator with an unknown rotator returns an error
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "nonexistent" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "rotator not found"

  Scenario: move_rotator with a disconnected rotator returns an error
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "rotator not connected"

  Scenario: move_rotator with an unknown train returns an error
    Given rp is running with an offline rotator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with train "nonexistent" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "train not found"

  Scenario: move_rotator on a train without a rotator returns an error
    Given rp is running with an offline camera-only train
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with train "main" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "no rotator"

  Scenario: move_rotator on a train with two rotators asks for the explicit id
    Given rp is running with two offline rotators inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with train "main" to angle 5.0
    Then the tool call should return an error
    And the error message should contain "pass rotator_id"

  Scenario: Moving a guiding-train rotator while guiding runs the rotate-while-guiding ladder
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a rotator on the simulator inside guiding train "guide"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 10.0
    Then the rotator result field "angle" should be 10.0 within 0.1
    And the stub guider should have received a pause request with full false
    And the stub guider should have received a "/star/reselect" request
    And the stub guider should have received a "/guiding/resume" request
    And the stub guider should have received a "/calibration/clear" request
    And the rotator result ladder field "calibration_cleared" should be true
    And the rotator result ladder field "phd2_has_rotator" should be false

  Scenario: A rotation inside the recalibration threshold keeps PHD2's calibration
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a rotator on the simulator inside guiding train "guide"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 2.0
    Then the rotator result field "angle" should be 2.0 within 0.1
    And the stub guider should have received a "/star/reselect" request
    And the stub guider should not have received a "/calibration/clear" request
    And the rotator result ladder field "calibration_cleared" should be false

  Scenario: A rotator PHD2 knows about needs no calibration clearing
    Given a running Alpaca simulator
    And a stub guider with a connected PHD2 rotator
    And rp is running with a rotator on the simulator inside guiding train "guide"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 90.0
    Then the rotator result ladder field "phd2_has_rotator" should be true
    And the rotator result ladder field "calibration_cleared" should be false
    And the stub guider should not have received a "/calibration/clear" request
    And the stub guider should have received a "/guiding/resume" request

  Scenario: Moving a rotator outside the guiding train never touches the guider
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a rotator on the simulator inside train "main"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 45.0
    Then the rotator result field "angle" should be 45.0 within 0.1
    And the stub guider should not have received a pause request
    And the rotator result should have no guiding ladder

  Scenario: Moving a guiding-train rotator while guiding is idle runs bare
    Given a running Alpaca simulator
    And a stub guider reporting guiding inactive
    And rp is running with a rotator on the simulator inside guiding train "guide"
    And an MCP client connected to rp
    When the MCP client calls "move_rotator" with rotator "main-rotator" to angle 45.0
    Then the rotator result field "angle" should be 45.0 within 0.1
    And the stub guider should not have received a pause request
    And the rotator result should have no guiding ladder
