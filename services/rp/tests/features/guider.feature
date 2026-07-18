@serial
Feature: Guider MCP tools
  The guiding tools (start_guiding, stop_guiding, dither, pause_guiding,
  resume_guiding, get_guiding_stats) proxy to the guider rp-managed
  service over HTTP. All quantities are guide-camera pixels. The
  guider is configured at equipment.mount.guiding — guiding is
  mount-scoped, so the block cannot exist without a mount. Settle
  parameters merge field by field: a per-call value wins over the
  guiding block's settle_* config default, and a field unset in both
  is omitted from the wire so the service's own settling config
  applies. The dither amount falls back from the pixels parameter to
  the guiding block's dither_pixels. The settle-blocking calls emit
  operation triples ending in settled
  (guide_started/guide_settled/guide_failed,
  dither_started/dither_settled/dither_failed), with the settle
  deadline carried on the started envelope when a settle timeout is
  known; stop_guiding emits the guide_stopped point event with reason
  "requested". Without a guiding block every tool errors with "guider
  not configured"; service errors propagate as code plus message.

  Scenario: Tool catalog includes the guider tools
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "start_guiding"
    And the tool list should include "stop_guiding"
    And the tool list should include "dither"
    And the tool list should include "pause_guiding"
    And the tool list should include "resume_guiding"
    And the tool list should include "get_guiding_stats"

  Scenario: start_guiding returns the settled RMS snapshot
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the guider result should contain "state" with value "guiding"
    And the guider result should contain "rms_ra_px" with number 0.3
    And the guider result should contain "rms_dec_px" with number 0.4
    And the guider result should contain "total_rms_px" with number 0.5
    And the guider result should contain "sample_count" with number 12

  Scenario: start_guiding with no settle configured sends no settle override
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the stub guider should have received a start request without a settle override

  Scenario: Config settle defaults are forwarded on start_guiding
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator and guider settle pixels 0.8 time "8s" timeout "40s"
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the stub guider should have received a start request with settle pixels 0.8 time "8s" timeout "40s"

  Scenario: Per-call settle parameters override the config defaults field by field
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator and guider settle pixels 0.8 time "8s" timeout "40s"
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with settle_pixels 1.5 and settle_timeout "20s"
    Then the stub guider should have received a start request with settle pixels 1.5 time "8s" timeout "20s"

  Scenario: recalibrate is forwarded on start_guiding
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with recalibrate true
    Then the stub guider should have received a start request with recalibrate true

  Scenario: start_guiding emits the guide operation events with the settle deadline
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And a test webhook receiver subscribed to "guide_started" and "guide_settled"
    And rp is running with a camera on the simulator and guider settle pixels 0.8 time "8s" timeout "40s"
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the test webhook receiver should receive a "guide_started" event
    And the test webhook receiver should receive a "guide_settled" event
    And the "guide_started" event carries the deadline fields
    And the "guide_settled" event payload should contain a "rms_ra_px"
    And the "guide_settled" event payload should contain a "total_rms_px"

  Scenario: dither forwards the amount and returns the re-settled snapshot
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "dither" with pixels 5.0 and ra_only true
    Then the guider result should contain "state" with value "guiding"
    And the stub guider should have received a dither request with amount_px 5.0 and ra_only true

  Scenario: dither falls back to the configured dither_pixels
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator and guider dither_pixels 3.5
    And an MCP client connected to rp
    When the MCP client calls "dither" with no arguments
    Then the stub guider should have received a dither request with amount_px 3.5 and ra_only false

  Scenario: dither with no amount available returns an error
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "dither" with no arguments
    Then the tool call should return an error
    And the error message should contain "dither_pixels"

  Scenario: dither emits the dither operation events
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And a test webhook receiver subscribed to "dither_started" and "dither_settled"
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "dither" with pixels 5.0 and ra_only true
    Then the test webhook receiver should receive a "dither_started" event
    And the test webhook receiver should receive a "dither_settled" event
    And the "dither_started" event payload should contain a "pixels"

  Scenario: stop_guiding confirms the stop and emits guide_stopped with reason requested
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And a test webhook receiver subscribed to "guide_stopped"
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "stop_guiding"
    Then the guider result should contain "state" with value "stopped"
    And the stub guider should have received a stop request
    And the test webhook receiver should receive a "guide_stopped" event
    And the "guide_stopped" event payload field "reason" should be "requested"

  Scenario: pause_guiding forwards full and resume_guiding resumes
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "pause_guiding" with full true
    Then the guider result should contain "state" with value "paused"
    And the stub guider should have received a pause request with full true
    When the MCP client calls "resume_guiding"
    Then the guider result should contain "state" with value "resumed"
    And the stub guider should have received a resume request

  Scenario: get_guiding_stats returns the full statistics snapshot
    Given a running Alpaca simulator
    And a stub guider returning canned guiding stats
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_guiding_stats"
    Then the guider result should contain "app_state" with value "Guiding"
    And the guider result should contain "rms_ra_px" with number 0.3
    And the guider result should contain "snr" with number 25.0
    And the guider result should contain "star_mass" with number 5432.0
    And the guider result should contain "sample_count" with number 12

  Scenario Outline: Guider tools without a configured guider return an error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls the guider tool "<tool>" with empty arguments
    Then the tool call should return an error
    And the error message should contain "guider not configured"

    Examples:
      | tool              |
      | start_guiding     |
      | stop_guiding      |
      | pause_guiding     |
      | resume_guiding    |
      | get_guiding_stats |

  Scenario: dither without a configured guider returns an error
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "dither" with pixels 5.0 and ra_only false
    Then the tool call should return an error
    And the error message should contain "guider not configured"

  Scenario: Service unreachable error when the guider URL points at an unbound port
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator and a guider pointing at an unbound port
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the tool call should return an error
    And the error message should contain "service unreachable"

  Scenario Outline: Guider service structured errors propagate verbatim
    Given a running Alpaca simulator
    And a stub guider returning error code "<code>" with message "<message>"
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the tool call should return an error
    And the error message should contain "<code>"
    And the error message should contain "<message>"

    Examples:
      | code             | message                          |
      | not_guiding      | PHD2 is not guiding              |
      | guide_failed     | PHD2 rejected the guide command  |
      | settle_timeout   | settle did not complete in time  |
      | phd2_unreachable | PHD2 connection lost             |
      | internal         | broken pipe                      |

  Scenario: A failing start_guiding emits guide_failed
    Given a running Alpaca simulator
    And a stub guider returning error code "guide_failed" with message "no guide star"
    And a test webhook receiver subscribed to "guide_failed"
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "start_guiding" with no arguments
    Then the tool call should return an error
    And the test webhook receiver should receive a "guide_failed" event
    And the "guide_failed" event payload should contain a "error"
