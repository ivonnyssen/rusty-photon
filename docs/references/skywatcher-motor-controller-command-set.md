# Sky-Watcher Motor Controller Command Set

> **Authoritative source:** [skywatcher_motor_controller_command_set.pdf](https://inter-static.skywatcher.com/downloads/skywatcher_motor_controller_command_set.pdf)
> (Sky-Watcher application notes, no version number on the file). The
> file below is a clean markdown rendering of the same content for
> in-tree reference; if it disagrees with the upstream PDF, the PDF wins.
> All material here is © Sky-Watcher and reproduced for engineering
> reference under fair-use; the canonical URL is the public-facing copy.

This is the wire-protocol specification used by:

- The [`star-adventurer-gti`](../services/star-adventurer-gti.md) ASCOM
  Telescope driver (USB at 115200 baud, or UDP/11880 over WiFi).
- The `skywatcher-motor-protocol` crate (codec; transport-agnostic).

The same protocol is shared by Sky-Watcher mounts including (per the EQMOD
project's published compatibility list) EQ4, EQ5, HEQ5, NEQ6, EQ6-R Pro,
EQ8, EQ8-R, EQ8-Rh, EQM-35 Pro, AZ-EQ5GT, AZ-EQ6GT, AZ-GTi, Star
Adventurer GTi, HDX110, and Orion-rebadged variants (Sirius Pro AZ/EQ-G,
Atlas Pro AZ/EQ-G).

---

## 1. Motor Speed Control

The motor controller has a hardware timer **T1** that generates stepping
pulses for stepper motors (or reference positions for servomotors). T1's
input clock frequency plus its preset value determine slew speed.

When T1 generates an interrupt, the controller might:

- Drive the motor **1 step** (1 micro-step or 1 encoder tick) for low-speed
  slewing.
- Drive the motor **up to 32 steps** for high-speed slewing. (Firmware
  v2.x only — firmware v3.x and above always advance 1 step per
  interrupt.)

## 2. Two Motion Modes

**GOTO mode.** The master device tells the motor controller the desired
destination, then sends a "Start" command. The controller moves the motor
to that destination. The master can poll status, position, and cancel the
slew during the GOTO.

**Speed (Tracking) mode.** The master computes a T1 preset value for the
desired speed and sends it to the controller, then sends "Start". The
controller drives the motor at that constant speed. The master can poll
status, position, and stop the slew.

The mode for the next "Start" is selected by a separate command. The motor
should be at full-stop status before changing motion mode.

When a motor stops automatically (i.e. arrives at GOTO target), the
controller returns to Speed mode by default.

A typical slewing session:

1. Confirm motor is at full stop. If not, stop it.
2. Set motion mode.
3. Set parameters (destination or T1 preset).
4. Send "Start".
5. For GOTO: poll status until motor stops (= arrived).
   For Speed: send "Stop" to end the session.

## 3. Master-Side Calculations

A Sky-Watcher motor controller does not perform complex calculations.
The master device does, using values it queries from the controller.

### Encoder counts ↔ angle

The controller counts encoder ticks. The master inquires **CPR** (Counts
Per Revolution) from the controller and converts angle ↔ counts.
**CPR may differ between the two axes of a mount.**

### T1 preset for tracking-mode speed

The master inquires **TMR_Freq** (T1 input clock frequency).

```
Speed_CountsPerSec = Speed_DegPerSec × CPR / 360
T1_Preset          = TMR_Freq / Speed_CountsPerSec
                   = TMR_Freq × 360 / (Speed_DegPerSec × CPR)
```

### T1 preset for high-speed slewing

When required slew speed is high (e.g. > 128× sidereal), T1's preset can
become unworkably small. The controller offers a high-speed mode that
moves **N micro-steps per T1 interrupt** instead of 1, where **N** is a
fixed mount-specific value (typically 16, 32, or 64) inquired from the
controller.

```
T1_Preset_HighSpeed = N × TMR_Freq × 360 / (Speed_DegPerSec × CPR)
```

When entering Speed mode the master must tell the controller whether it
intends low-speed or high-speed slewing. In GOTO mode the controller
selects automatically.

## 4. Command Format

### Frame structure

```
Command:    : <cmd> <axis> <payload?> \r
Response:   = <payload?>             \r        (success)
Response:   ! <2-hex-errcode>        \r        (error)
```

- Every command starts with `:` (`0x3A`) and ends with carriage return
  `\r` (`0x0D`).
- If a second `:` is received before `\r`, the controller discards the
  partial frame and starts receiving a new one. (Useful for resync.)
- The controller processes the command and replies after it has read the
  complete frame.
- A normal response starts with `=`. An error response starts with `!`
  followed by 2 hex digits of error code.
- All bytes are ASCII.

### Command parts

- **1 byte** leading `:`
- **1 byte** command word (see command set table)
- **1 byte** channel word: `'1'` = RA/Az axis, `'2'` = Dec/Alt axis,
  `'3'` = both axes (where applicable)
- **1 to 6 bytes** of data payload — ASCII hex digits `'0'`–`'9'`,
  `'A'`–`'F'`
- **1 byte** trailing `\r`

### Response parts (normal)

- **1 byte** leading `=`
- **1 to 6 bytes** of data — ASCII hex digits
- **1 byte** trailing `\r`

### Response parts (error)

- **1 byte** leading `!`
- **2 bytes** of error code (ASCII hex digits)
- **1 byte** trailing `\r`

### Data encoding — 24-bit values are sent low byte first

For HEX value `0x123456`, the wire bytes are ASCII:

```
  "5" "6"   "3" "4"   "1" "2"
   ^^^^^     ^^^^^     ^^^^^
   byte 0    byte 1    byte 2
   (low)               (high)
```

Each byte's nibbles appear in normal high-then-low order; only the byte
order itself is reversed.

For 16-bit values: bytes 0 then byte 1 (low first).
For 8-bit values: a single byte's nibbles in normal order ("12" for
`0x12`).

### Data encoding — position offset 0x800000

Axis positions on the wire are biased by `+0x800000` so that the unsigned
hex representation can carry both signed-positive and signed-negative
encoder counts:

```
true encoder count 0       → wire 0x800000
true encoder count +0x1234 → wire 0x801234
true encoder count -0x1234 → wire 0x7FEDCC
```

The controller adds the bias when reporting; the master adds the bias
when sending. (See `:E`, `:S`, `:H`, `:j`, `:h`, etc.)

## 5. Command Set

Channel column conventions: `*1` = `'1'` / `'2'` / `'3'` (CH1, CH2, or
both). `*2` = `'1'` / `'2'` (CH1 or CH2 only). Literal `'1'` = CH1 only.

### Setters / motion commands

| Cmd | Letter | Channel | Data        | Response | Notes |
|---|---|---|---|---|---|
| Set Position                  | `E` | `*1` | 6 hex      | A, X | Motor must be full stopped |
| Initialization Done           | `F` | `*1` | —          | A, X | |
| Set Motion Mode               | `G` | `*1` | 2 hex flags| A, X | Motor must be full stopped. See [§6 Motion Mode bits](#6-motion-mode-flags-data-of-g) |
| Set Goto Target Increment     | `H` | `*2` | 6 hex      | A, X | |
| Set Brake Point Increment     | `M` | `*1` | 6 hex      | A, X | |
| Set Goto Target               | `S` | `*1` | 6 hex      | A, X | Motor must be full stopped |
| Set Step Period (T1 preset)   | `I` | `*1` | 6 hex      | A, X | Cannot change while motor is high-speed slewing |
| Set Long Goto Step Period     | `T` | `*1` | 6 hex      | A, X | |
| Set Brake Steps               | `U` | `*1` | 6 hex      | A, X | |
| Start Motion                  | `J` | `*1` | —          | A, X | |
| Stop Motion                   | `K` | `*1` | —          | A, X | Channel reverts to Tracking Mode after stop |
| Instant Stop                  | `L` | `*1` | —          | A, X | Channel reverts to Tracking Mode after stop |
| Set Sleep                     | `B` | `*1` | `'0'` wake / `'1'` sleep | A, X | |
| Set Aux Switch On/Off         | `O` | `*1` | `'0'` off / `'1'` on     | A, X | |
| Set AutoGuide Speed           | `P` | `*1` | `'0'`=1×, `'1'`=0.75×, `'2'`=0.5×, `'3'`=0.25×, `'4'`=0.125× | A, X | |
| Run Bootloader Mode           | `Q` | `*1` | `"55AA"`   | (none) | No response — caution |
| Set Polar Scope LED brightness| `V` | `*1` | 2 hex      | A, X | |
| Set Debug Flag                | `z` | `*1` | —          | (none) | |
| Extended Setting              | `W` | `*1` | 6 hex (ID + payload) | X | See [§7](#7-extended-setting--inquire) |

### Inquiries

| Cmd | Letter | Channel | Data | Response | Notes |
|---|---|---|---|---|---|
| Inquire Counts Per Revolution | `a` | `*2` | —     | B, X | per-axis CPR |
| Inquire Timer Interrupt Freq  | `b` | `'1'` | —     | B, X | TMR_Freq |
| Inquire Brake Steps           | `c` | `*2` | —     | B, X | |
| Inquire Goto Target Position  | `h` | `*2` | —     | B, X | |
| Inquire Step Period           | `i` | `*2` | —     | B, X | |
| Inquire Position              | `j` | `*2` | —     | B, X | with 0x800000 bias |
| Inquire Increment             | `k` | `*2` | `'0'` no-reset / `'1'` reset | B, X | |
| Inquire Brake Point           | `m` | `*2` | —     | B, X | |
| Inquire Status                | `f` | `*2` | —     | E, X | See [§8 Status response](#8-status-response-of-f) |
| Inquire High Speed Ratio      | `g` | `*2` | —     | D, X | the **N** factor |
| Inquire 1X Tracking Period    | `D` | `'1'` | —     | B, X | |
| Inquire Tele. Axis Position   | `d` | `*1` | —     | B, X | |
| Inquire Motor Board Version   | `e` | `*1` | —     | B, X | Last byte indicates EQ (`0`) vs AZ (`1`); see [§9](#9-motor-board-version-of-e) |
| Inquire PEC period            | `s` | `*1` | —     | B, X | |
| Extended Inquire              | `q` | `*1` | 6 hex (ID + payload) | X | See [§7](#7-extended-setting--inquire) |

### EEPROM access

| Cmd | Letter | Channel | Data | Response |
|---|---|---|---|---|
| Set EEPROM Address    | `C` | `'1'` | 4 hex address | (none) |
| Set EEPROM Value      | `N` | `'1'` | 2 hex value   | (none) |
| Inquire EEPROM Value  | `n` | `'1'` | —             | (B-format response) |

### Register access

| Cmd | Letter | Channel | Data | Response |
|---|---|---|---|---|
| Set Register Address  | `A` | `*1` | 2 hex address | (none) |
| Set Register Value    | `R` | `*1` | 2 hex value   | (none) |
| Inquire Register Value| `r` | `*1` | —             | (response) |

### Response payload widths

| Tag | Format       | Data width |
|---|---|---|
| A   | `=\r`        | 0 bytes    |
| B   | `=<6 hex>\r` | 24 bits (low byte first, with 0x800000 bias for position commands) |
| C   | `=<4 hex>\r` | 16 bits (low byte first) |
| D   | `=<2 hex>\r` | 8 bits     |
| E   | `=<3 hex>\r` | 3 status nibbles — see [§8](#8-status-response-of-f) |
| X   | `!<2 hex>\r` | 8-bit error code |

## 6. Motion Mode Flags (data of `:G`)

The `:G` data field is two ASCII hex nibbles. The first nibble selects
GOTO vs Tracking and the speed regime; the second nibble selects axis
direction and (for Alt/Az mounts) the hemisphere.

The exact bit semantics differ between Goto and Tracking flavours and
between firmware revisions; consult the upstream PDF page §"Set Motion
Mode" plus the [INDI `indi-eqmod` source][indi-eqmod] for the
authoritative bit map. Driver code should encapsulate motion-mode
construction behind a typed builder rather than spread raw nibble values
through the codebase.

[indi-eqmod]: https://github.com/indilib/indi-3rdparty/tree/master/indi-eqmod

The general shape (low → high bit, nibble 0):

- **B0:** `0` = GOTO mode, `1` = Tracking mode
- **B1:** speed regime — exact meaning depends on B0
  (in Tracking: `0` = fast, `1` = slow; in GOTO: `0` = slow, `1` = fast)
- **B2:** medium-speed flag (Tracking only) / Coarse Goto flag (GOTO only)
- **B3:** 1× Slow Goto flag (GOTO only)

Nibble 1:

- **B0:** `0` = CW, `1` = CCW
- **B1:** `0` = North hemisphere, `1` = South hemisphere

After `:K` (Stop) or `:L` (Instant Stop), the channel always reverts to
Tracking Mode for the next Start.

## 7. Extended Setting / Inquire

The `:W` (set) and `:q` (inquire) commands carry a 6-hex-digit payload
where the leading 6 bits are an **ID** code and the remainder is
ID-specific.

### Extended Inquire (`:q`)

| ID       | Function                         | Returned bytes |
|---|---|---|
| `000000` | Inquire Axis (Original) Indexer Position | 6 hex |
| `000001` | Inquire Status EX                | 6 hex (see below) |

`Inquire Status EX` (ID `000001`) returns 6 hex nibbles encoding multiple
capability flags:

- Byte 0: B0 = PEC Training on/off, B1 = PEC Tracking on/off
- Byte 1: B0 = Supports dual encoder, B1 = Supports PPEC,
  B2 = Supports original-position indexer, B3 = Supports EQ/AZ mode
- Byte 2: B0 = Has polar-scope LED, B1 = Two axes must start separately,
  B2 = Supports tracking-torque selection
- Bytes 3–5: reserved

### Extended Setting (`:W`)

| ID       | Function                                |
|---|---|
| `000000` | Start PEC Training                      |
| `000001` | Cancel PEC Training                     |
| `000002` | Start PEC Tracking                      |
| `000003` | Cancel PEC Tracking                     |
| `000004` | Enable Dual Encoder                     |
| `000005` | Disable Dual Encoder                    |
| `000006` | Disable full current (torque) at low speed |
| `000106` | Enable  full current (torque) at low speed |
| `xxxx07` | Set Stride for Slewing (`xxxx` = stride) |
| `000008` | Reset Axis Indexer Position             |
| `000009` | Write flash buffer in RAM to flash ROM  |

## 8. Status Response (of `:f`)

Returns 3 hex nibbles; each nibble is one status byte.

### Status byte 0 (mode, direction, speed regime)

| Bit | Meaning |
|---|---|
| B0  | `0` = Goto, `1` = Tracking |
| B1  | `0` = CW, `1` = CCW |
| B2  | `0` = Slow, `1` = Fast |

### Status byte 1 (motor state)

| Bit | Meaning |
|---|---|
| B0  | `0` = Stopped, `1` = Running |
| B1  | `0` = Normal, `1` = Blocked |

### Status byte 2 (initialization, sensors)

| Bit | Meaning |
|---|---|
| B0  | `0` = Not initialized, `1` = Init done |
| B1  | `0` = Level switch off, `1` = Level switch on |

## 9. Motor Board Version (of `:e`)

The `:e<axis>` reply payload (6 hex digits, low byte first) decodes as:

```
byte 0 — minor firmware version
byte 1 — major firmware version
byte 2 — mount-family code: last hex digit is 0 = EQ, 1 = AZ
```

Empirically on a Star Adventurer GTi: `=03300C\r` →
bytes `0x03`, `0x30`, `0x0C` → fw `0x30.0x0C` on mount-family `0x03`
(EQ family; the `3` low-nibble of the last byte does not match the
"`0` = EQ, `1` = AZ" rule literally — the rule applies to the **last
hex character** of the wire string, not the high byte's low nibble; see
PDF §*6 footnote).

## 10. Error Codes

| Code | Name                       |
|---|---|
| `0` | Unknown Command             |
| `1` | Command Length Error        |
| `2` | Motor not Stopped           |
| `3` | Invalid Character           |
| `4` | Not Initialized             |
| `5` | Driver Sleeping             |
| `7` | PEC Training is running     |
| `8` | No Valid PEC data           |

Codes `6`, `9`, `A`–`F` are reserved.

## 11. Hardware

### UART (serial transport)

- **9600 bps**, 8 data bits, 1 start bit, 1 stop bit, no parity.
  - In practice the Star Adventurer GTi's USB-CDC port also accepts
    **115200 bps**, which the EQMOD project recommends and which our
    driver uses by default.
- Signal level: 5 V or 3.3 V.
- **EQ mounts** have separate TX and RX lines. The controller sends its
  response immediately after receiving and processing the command.
- **Alt/Az mounts** typically have TX and RX wired together with a
  separate `Drop` line indicating bus-busy. The master pulls `Drop` low
  during transmission and keeps it low until either the response is
  received in full or a timeout expires. The controller pulls TX high
  with a 5.1 kΩ–10 kΩ resistor only — soft enough that the master can
  drive it low without contention.

### WiFi transport

- The same protocol runs on the **SynScan WiFi dongle** and on mounts
  with a **built-in WiFi module**.
- The WiFi dongle/module hosts a **UDP server on port 11880**.
- **One UDP packet per command, one UDP packet per response.** The
  packet must contain exactly one well-formed frame; trailing junk
  causes silent drop.
- In **AP mode**, the dongle/module IP is `192.168.4.1`. In **station
  mode**, the upstream router allocates the IP via DHCP.

## 12. Useful Resources

- [Sample code archive (Google Code, archived)](https://code.google.com/archive/p/skywatcher/)
- [Sky-Watcher application development page (canonical PDFs)](http://www.skywatcher.com/download/manual/application-development/)
- [INDI `indi-eqmod` source](https://github.com/indilib/indi-3rdparty/tree/master/indi-eqmod)
- [EQMOD project](https://eq-mod.sourceforge.net/)
