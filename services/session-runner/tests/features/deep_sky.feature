@serial
Feature: Deep-sky workflow document
  The shipped deep_sky.json drives a full night cycle against rp's real
  planner: unpark and start tracking, then a dispatch loop that asks
  get_next_target after every frame — on a target change it slews,
  optionally plate-solve-centers and auto-focuses, then captures one
  frame per pass — ending at the max_frames budget (or dawn) and
  optionally parking. Because the planner evaluates real ephemeris at
  wall-clock now, every scenario computes its observing site to fit the
  clock: an equatorial site at the anti-solar longitude is always in
  deep astronomical night, and celestial-equator targets placed by hour
  angle sink at a constant 0.25 degrees per minute — which makes "this
  target drops below its floor in N seconds" exact. The dawn scenario
  flips the trick: a site 45 degrees west of the sub-solar longitude
  has a risen, still-climbing morning sun at any moment, so the planner
  answers end_of_session. The simulated mount
  is taught the same site (rp refuses a mount whose reported site
  disagrees with config) and synced onto the first target so document
  slews stay sub-degree.

  Exposure counts include every capture rp makes on the document's
  behalf: each center_on_target iteration captures one solving frame,
  and each auto_focus sweep captures one frame per grid position, on
  top of the light frames the capture loop takes.

  Scenario: A full night cycle runs start to finish and parks the mount
    Given a running Alpaca simulator
    And an observing site where it is astronomical night with one planner target
    And the simulated mount matches the site and points at the first target
    And a stub plate solver echoing the first target
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure       | 500ms |
      | max_frames     | 3     |
      | focus          | false |
      | centering      | true  |
      | park_on_finish | true  |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    Then the session ends within 120 seconds
    And the blackboard is deleted within 10 seconds
    And the SSE stream should show exactly 1 "unpark_complete" event
    And the SSE stream should show exactly 1 "slew_complete" event
    And the SSE stream should show exactly 1 "centering_complete" event
    # 3 light frames + 1 centering solve frame (converged on iteration 1).
    And the SSE stream should show exactly 4 "exposure_complete" events
    And the SSE stream should show exactly 1 "park_complete" event

  Scenario: The planner's exposure plan drives the capture duration
    Given a running Alpaca simulator
    And an observing site where it is astronomical night with one planner target whose exposure plan is a single unfiltered 2-second frame
    And the simulated mount matches the site and points at the first target
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | max_frames     | 2     |
      | focus          | false |
      | centering      | false |
      | park_on_finish | false |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    # `exposure` is deliberately not supplied: its default is 300s, so
    # the session can only finish this fast if the planner-returned 2s
    # plan is what reaches the camera.
    Then the session ends within 90 seconds
    And the SSE stream should show exactly 2 "exposure_complete" events

  Scenario: A session started after dawn ends immediately with no frames
    Given a running Alpaca simulator
    And an observing site where the morning sun has risen and one planner target sits below its floor
    And the simulated mount matches the site and points at the first target
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure       | 2s    |
      | max_frames     | 2     |
      | focus          | false |
      | centering      | false |
      | park_on_finish | false |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    # The planner sees a bright rising Sun and no viable target, so its
    # first answer is end_of_session and the document ends the session
    # before slewing or capturing anything. Before rp #465 the reason
    # was wait_for_twilight and the document's zero-frames heuristic
    # read it as dusk — it would have waited forever.
    Then the session ends within 30 seconds
    And the SSE stream should show exactly 0 "slew_complete" events
    And the SSE stream should show exactly 0 "exposure_complete" events

  Scenario: A target sinking below its altitude floor switches the dispatch loop to the next target
    Given a running Alpaca simulator
    And an observing site where it is astronomical night and the first of two planner targets sinks below its floor after 120 seconds
    And the simulated mount matches the site and points at the first target
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure       | 2s    |
      | max_frames     | 0     |
      | focus          | false |
      | centering      | false |
      | park_on_finish | false |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    # The first slew acquires the sinking target; the second acquires
    # the backup once the planner drops the first below its floor.
    Then a second "slew_complete" event should be observed within 300 seconds
    And at least 1 "exposure_complete" event should precede the second "slew_complete" on the stream
    And at least 1 "exposure_complete" event should follow the second "slew_complete" on the stream

  Scenario: The refocus trigger re-runs auto_focus after the configured number of frames
    Given a running Alpaca simulator
    And an observing site where it is astronomical night with one planner target
    And the simulated mount matches the site and points at the first target
    And rp is running with a camera, a mount, a focuser, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure          | 500ms |
      | max_frames        | 3     |
      | focus             | true  |
      | refocus_every     | 2     |
      | refocus_hfr_factor | 0    |
      | centering         | false |
      | park_on_finish    | false |
      | focus_exposure    | 100ms |
      | focus_step_size   | 100   |
      | focus_half_width  | 200   |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    Then the session ends within 180 seconds
    # One sweep at acquisition, one from the refocus-after-frames
    # trigger after the second light frame. focus_started is emitted at
    # sweep begin, so the count holds whether or not the V-curve fit
    # succeeds against the simulator's images (the document try-wraps
    # auto_focus and continues on a failed fit).
    And the SSE stream should show exactly 2 "focus_started" events

  Scenario: A due meridian flip re-slews to the target between exposures, never during one
    Given a running Alpaca simulator
    And an observing site where it is astronomical night with one planner target
    And the simulated mount matches the site and points at the first target
    # meridian_margin is far above any real time_to_flip, so the 30s
    # meridian poll's first cycle fires the trigger deterministically
    # mid-loop.
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure        | 2s     |
      | max_frames      | 20     |
      | focus           | false  |
      | centering       | false  |
      | park_on_finish  | false  |
      | meridian_margin | 100000 |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    Then the session ends within 240 seconds
    # The acquisition slew plus at least one flip re-slew.
    And the SSE stream should show at least 2 "slew_complete" events
    And no "slew_complete" event should fall between an "exposure_started" and its "exposure_complete"

  Scenario: A safety interruption pauses the session and the resumed run re-acquires the target
    Given a running Alpaca simulator
    And a safety monitor guards the session
    And an observing site where it is astronomical night with one planner target
    And the simulated mount matches the site and points at the first target
    And a stub plate solver echoing the first target
    And rp is running with a camera, a mount, and the session-runner orchestrator running the "deep_sky" workflow with parameters:
      | exposure       | 2s    |
      | max_frames     | 4     |
      | focus          | false |
      | centering      | true  |
      | park_on_finish | false |
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    And the deep-sky session has captured at least 2 frames
    And the safety monitor reports unsafe
    Then rp reports the session as "interrupted" within 5 seconds
    And the blackboard is kept
    When the safety monitor reports safe again
    Then rp reports the session as "active" within 5 seconds
    And the blackboard is deleted within 120 seconds
    # Two acquisitions (initial + the recovery re-acquisition the
    # document performs on params._recovery) — that is the resume
    # contract this scenario pins.
    And the SSE stream should show exactly 2 "centering_complete" events
    # 4 light frames + 2 centering solve frames, plus at most one frame
    # whose exposure completed just before the safety abort landed.
    And the SSE stream should show between 6 and 7 "exposure_complete" events
    And the SSE stream should show exactly 2 "safety_changed" events
