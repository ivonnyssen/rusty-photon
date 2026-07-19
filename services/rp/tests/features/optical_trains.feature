@serial
Feature: Optical trains configuration
  equipment.optical_trains models each camera's light path as an ordered
  list of roster device ids, objective side first, terminating in a
  camera. Membership expresses coupling and position expresses optical
  order; rp derives focus pairing, rotation effects, and the exposure
  document's optics block from the lists. Guiding is mount-scoped: the
  guider service is configured at equipment.mount.guiding, and a train
  with purpose "guiding" requires that block. Per-field invariants
  (purpose enum, focal-length positivity) are rejected at parse with
  HTTP 400; cross-array graph rules are validated on PUT /api/config as
  HTTP 200 status "invalid" with dotted error paths, leaving the file
  untouched. The retired pre-train keys (top-level guider,
  cameras[].focal_length_mm, focusers[]/filter_wheels[].camera_id) are
  unknown fields and fail at parse.

  Scenario: Configured optical trains round-trip through GET /api/config
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    Then the config response status should be 200
    And the fetched config field "/equipment/optical_trains/0/id" should be "main"
    And the fetched config field "/equipment/optical_trains/0/purpose" should be "imaging"
    And the fetched config field "/equipment/optical_trains/1/purpose" should be "guiding"
    And the fetched config field "/equipment/mount/guiding/url" should be "http://127.0.0.1:1"

  Scenario: PUT /api/config accepts the reference optical trains unchanged
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config unchanged
    Then the config response status should be 200
    And the apply status should be "ok"

  Scenario: A train device missing from the roster is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": ["ghost-focuser", "main-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.0.devices.0"

  Scenario: A train not terminating in a camera is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": ["main-focuser"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.0.devices.0"

  Scenario: A train with no devices is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": [] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.0.devices"

  Scenario: A camera before the end of a train is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": ["main-cam", "guide-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.0.devices.0"

  Scenario: A camera terminating two trains is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main",   "devices": ["main-focuser", "main-cam"] },
        { "id": "second", "devices": ["guide-focuser", "main-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.1.devices.1"

  Scenario: A device repeated within one train is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": ["main-focuser", "main-focuser", "main-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.0.devices.1"

  Scenario: Duplicate train ids are rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main", "devices": ["main-focuser", "main-cam"] },
        { "id": "main", "devices": ["guide-focuser", "guide-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.1.id"

  Scenario: A second guiding train is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main",  "purpose": "guiding", "devices": ["main-focuser", "main-cam"] },
        { "id": "guide", "purpose": "guiding", "devices": ["guide-focuser", "guide-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.1.purpose"

  Scenario: A guiding train without mount guiding configuration is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/mount/guiding" to "null"
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains.1.purpose"

  Scenario: Contradictory shared-device order across trains is rejected
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains" to:
      """
      [ { "id": "main",  "devices": ["main-focuser", "guide-focuser", "main-cam"] },
        { "id": "guide", "devices": ["guide-focuser", "main-focuser", "guide-cam"] } ]
      """
    Then the config response status should be 200
    And the apply status should be "invalid"
    And the apply errors should name path "equipment.optical_trains"

  Scenario: A non-positive train focal length is rejected at parse
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains/0/focal_length_mm" to "-100.0"
    Then the config response status should be 400
    And the config response body should contain "focal_length_mm must be a positive finite number"

  Scenario: An unknown train purpose is rejected at parse
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains/0/purpose" to "solar"
    Then the config response status should be 400
    And the config response body should contain "unknown variant `solar`"

  Scenario Outline: A train auto_focus block with a non-positive sweep field is rejected at parse
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after setting "/equipment/optical_trains/0/auto_focus/<field>" to "<value>"
    Then the config response status should be 400
    And the config response body should contain "<named>"

    Examples:
      | field      | value | named                                          |
      | step_size  | 0     | auto_focus.step_size must be a positive integer  |
      | half_width | -5    | auto_focus.half_width must be a positive integer |

  Scenario Outline: Retired pre-train config keys are rejected as unknown fields
    Given a temp rp config with the reference optical trains
    And rp is started with that config file
    When I GET /api/config
    And I PUT /api/config with the fetched config after inserting "<pointer>" set to "<value>"
    Then the config response status should be 400
    And the config response body should contain "<named>"

    Examples:
      | pointer                              | value                        | named           |
      | /guider                              | {"url": "http://127.0.0.1:1"} | guider          |
      | /equipment/cameras/0/focal_length_mm | 1000.0                       | focal_length_mm |
      | /equipment/focusers/0/camera_id      | "main-cam"                   | camera_id       |
      | /equipment/filter_wheels/0/camera_id | "main-cam"                   | camera_id       |

  Scenario: Capture derives the optics block from the camera's train focal length
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator in an imaging train with focal length 1000.0
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And I fetch the document for the captured document_id
    Then the document response status should be 200
    And the document optics focal length should be 1000.0
    And the document optics pixel scale should equal 206.265 times pixel size over focal length

  Scenario: Capture through a camera outside any train carries no optics block
    Given a running Alpaca simulator
    And rp is running with a camera on the simulator
    And an MCP client connected to rp
    When the MCP client calls "capture" with camera "main-cam" for 1000 ms
    And I fetch the document for the captured document_id
    Then the document response status should be 200
    And the document body should not contain "optics"
