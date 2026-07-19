@serial
Feature: refocus_train compound tool
  refocus_train expands one refocus trigger on an optical train into
  the dependency-ordered auto-focus sequence the train model derives:
  shared focusers upstream-first, each run in the train where that
  focuser is terminal, then the train's own terminal focuser. Sweep
  parameters come from each run train's auto_focus config block —
  there are no per-call sweep parameters — and every step is one full
  V-curve run emitting the focus_started / focus_complete /
  focus_failed triple exactly as auto_focus does, wrapped in a
  refocus_started / refocus_complete / refocus_failed operation
  triple. When mount guiding is configured and a step moves a focuser
  belonging to the guiding train, rp reads the guider's stats and, if
  guiding is active, pauses guide corrections (output only) before
  the first step and resumes after the last — also on the failure
  path. A stats read that fails or reports not-guiding skips the
  handshake instead of blocking the refocus. Auto-focus steps that
  would run in the guiding train itself are refused until the guiding
  integration phase: guide-train AF reads PHD2 metrics and never
  captures through the guide camera.

  The simulator's camera image is focuser-independent (flat HFR), so
  a real sweep's parabolic-fit outcome is not deterministic here —
  exactly as in auto_focus.feature, the simulator scenarios assert
  the deterministic artifacts (captures on disk, events, the guider
  handshake) and leave the success payload to the unit tests over
  the V-curve fixture registry.

  Scenario: Tool catalog includes refocus_train
    Given a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "refocus_train"

  Scenario: A refocus expands to the terminal focuser's V-curve through the train's camera
    Given rp's data_directory is pinned to a fresh tempdir
    And a running Alpaca simulator
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then 5 FITS files should exist in the pinned data directory

  Scenario: Refocus pauses and resumes guiding around a guiding-coupled focuser
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block and an offline guiding train sharing the focuser
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then the stub guider should have received a pause request with full false
    And the stub guider should have received a resume request

  Scenario: The handshake is skipped when the guider reports not guiding
    Given a running Alpaca simulator
    And a stub guider reporting guiding inactive
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block and an offline guiding train sharing the focuser
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then the stub guider should not have received a pause request

  Scenario: The handshake is skipped when the guider stats are unreachable
    Given rp's data_directory is pinned to a fresh tempdir
    And a running Alpaca simulator
    And an unreachable guider configuration
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block and an offline guiding train sharing the focuser
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then 5 FITS files should exist in the pinned data directory

  Scenario: A failing step stops the sequence and still resumes guiding
    Given a stub guider returning canned guiding stats
    And rp is running with offline main and guiding trains sharing the focuser
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then the tool call should return an error
    And the error message should contain "step 1 (focuser 'main-focuser' in train 'main')"
    And the stub guider should have received a pause request with full false
    And the stub guider should have received a resume request

  Scenario: refocus_train emits the operation envelope and the per-step focus events
    Given rp's data_directory is pinned to a fresh tempdir
    And a running Alpaca simulator
    And a test webhook receiver subscribed to "refocus_started" and "focus_started"
    And rp is running with a camera and a focuser on the simulator in train "main" with the standard auto_focus block
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main" and reason "temperature_drift"
    Then the test webhook receiver should receive a "refocus_started" event
    And the test webhook receiver should receive a "focus_started" event
    And the "refocus_started" event payload field "train_id" should be "main"
    And the "refocus_started" event payload field "reason" should be "temperature_drift"
    And the "refocus_started" event payload should contain a "steps"
    And the "refocus_started" event payload should contain a "guiding_paused"

  Scenario: Refocusing the guiding train is refused until the guiding integration
    Given rp is running with an offline reference rig and mount guiding
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "guide"
    Then the tool call should return an error
    And the error message should contain "guiding train"

  Scenario: refocus_train with an unknown train returns an error
    Given rp is running with an offline focuser train without an auto_focus block
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "nonexistent"
    Then the tool call should return an error
    And the error message should contain "train not found"

  Scenario: refocus_train on a train without focusers returns an error
    Given rp is running with an offline camera-only train
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then the tool call should return an error
    And the error message should contain "no focuser"

  Scenario: refocus_train without an auto_focus block on the run train returns an error
    Given rp is running with an offline focuser train without an auto_focus block
    And an MCP client connected to rp
    When the MCP client calls "refocus_train" with train "main"
    Then the tool call should return an error
    And the error message should contain "auto_focus"
