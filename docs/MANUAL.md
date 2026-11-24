# pic-sim Manual

Revision for **pic-sim 0.2.17** and **basil-sim 0.2.17** (one workspace version). Simulated families: **PIC18** (PIC18F27Q10, the PIC18F04/05/06/14/15/16Q41 and the PIC18(L)F23/24/25/26/43/44/45/46K22) and **PIC16 enhanced mid-range** (PIC16F131xx, 132xx, 152xx, 171xx, 180x6 and 183xx).

This manual describes what the simulator does today. It is organised in the order you meet the tools: what the simulator is, how to build and start it, how to prepare a program for it, the graphical front end `pic-sim`, the command-line front end `basil-sim`, the stimulus file that describes the outside world, the simulated board (LEDs and OLED), and finally what is and is not modelled. Anything not listed here is not supported.

Status: instruction timing follows the data sheets; peripheral behavior and timing follow the cited rules and explicit modelling assumptions. Some models have also been compared with Microchip MDB (the MPLAB X simulator). It has **not been compared with silicon**. Where the data sheet is silent the simulator makes an assumption; each one is numbered `[A#]` and written down with its source in `docs/FACTS-SIM.md`. The few places where the simulator deliberately disagrees with MDB are numbered `D-SIM-#` in the same file.

Related documents: `docs/FACTS-SIM.md` (every timing rule with its data sheet page), `docs/SIMULATOR-PLAN.md` (architecture and milestones), `docs/DEVICE-FORMAT.md` (the device description files), `docs/REVIEW-FINDINGS.md`. The compiler that produces the programs is **basil**; its manual is `Basil_Manual.pdf` in the Basic_Compiler project.

---

# Contents

1. Overview
2. Installing and starting
3. Preparing a program
4. Supported devices
5. The graphical simulator: pic-sim
6. The command-line simulator: basil-sim
7. The stimulus file
8. The simulated board: LEDs and OLED
9. What is simulated
10. Troubleshooting
11. Appendix A: quick reference
12. Appendix B: example session

---

# 1. Overview

## What it is

The workspace contains one simulation library and two programs that use it:

| Program | Kind | Use it to |
|---|---|---|
| `pic-sim` | Window (egui) | Load a program, run it, step it by instruction or by BASIC or C source line, set breakpoints, watch registers, memory and variables, wire LEDs and an OLED display to pins, time code with a stopwatch |
| `basil-sim` | Command line | Run a program for a number of cycles or until it halts, then print variables, serial/SPI/I2C traffic, pin changes, LED history and the OLED picture; drive scripted tests and benchmarks |

Both load the `.hex` file that the **basil** compiler writes, and both read the same **stimulus** file (a TOML file that says what the outside world does to the pins, serial receiver, ADC and so on). Both use the same simulated parts, so a result seen in one is the result in the other.

## What "cycle exact" means

The simulator executes the instructions of the part exactly as the hex file encodes them and counts **instruction cycles** (one cycle is four oscillator clocks, `Fosc / 4`). The peripherals are modelled to the cycle as well: a timer prescaler, the delay before an interrupt is taken, the time a UART needs per character or an ADC conversion needs per sample. The cycle counter in the window and in `basil-sim`'s `stop:` line reports the model's predicted cycles; dividing by `Fosc / 4` gives simulated time. `FACTS-SIM.md` identifies timing assumptions, approximations and known MDB differences; peripheral timing has not all been measured on silicon.

Because time is simulated, it is independent of how fast the host runs the simulation. A program that waits a simulated second can finish in milliseconds of host time (speed *Max*), or be slowed to real time or a fraction of it (section 5, *Run speed*).

## What it is not

* It is not a hardware debugger. The GUI edits simulated registers and memory; these changes affect only the simulated part.
* Only peripherals that are modelled behave. A register belonging to a peripheral without a model is plain storage, and both programs tell you which such registers the program touched (section 9).
* Analogue behaviour is not simulated. The ADC converts a code that you supply; pull-up resistance, drive strength and input thresholds are not modelled.

---

# 2. Installing and starting

## Build

You need a Rust toolchain that supports the 2024 edition (Rust 1.85 or newer). From the top of the workspace:

```
cargo build --release
```

This produces three programs in `target/release/`:

| File | Program |
|---|---|
| `pic-sim` | the graphical simulator |
| `basil-sim` | the command-line simulator |
| `basil-sim-dap` | the stdio debug adapter for editors; see [DAP setup](DAP.md) |

`cargo test --release` runs the test suite (core, peripherals, board, debug information). `pic-sim --version` and `basil-sim --version` print the version, which is also shown in the window title.

On Linux the window needs the usual graphics libraries for an `egui` program (X11 or Wayland and OpenGL); file dialogs are provided by the `rfd` crate.

## Where the device descriptions are found

The simulator needs one description file per part, `devices/<part>.toml` (section 4). Both programs look for the folder in this order and use the first that exists:

1. `--devices DIR` (`basil-sim` only);
2. the folder named by the environment variable `PICSIM_DEVICES` (when it is set it is used even if it does not exist, so a mistyped path is an error rather than another set of parts);
3. `devices/` in the folder that holds the executable;
4. `../share/picsim/devices` from the executable's folder (for a binary installed in `<prefix>/bin`);
5. `<workspace>/devices`, the path the programs were built from.

So a binary run from where `cargo` built it finds the workspace's folder as before. To install or move it, copy `devices/` beside it, or set `PICSIM_DEVICES`. If `pic-sim`'s device box is empty, none of these folders was found.

## Starting

```
pic-sim [prog.hex | prog.elf] [--device PART]
basil-sim prog.hex|prog.elf [--device PART] [options]
```

With no argument `pic-sim` opens empty; type a path or use *Browse hex/elf/cof…*. `basil-sim` needs a program. The default part is `18F27Q10` for `basil-sim`; `pic-sim` starts with the first PIC18 part in its list. Because a program built with `basil -g` records the part it was built for, you normally do not need `--device` in the window (section 5).

---

# 3. Preparing a program

The simulator runs what **basil** produces, and C programs built with **MPLAB XC8** (section 3, *C programs from XC8*). To compile a BASIC program:

```
basil blink.bas            # writes blink.hex, blink.S, blink.map
basil -g blink.bas         # also writes blink.elf: lines, variables, routines
basil --dual blink.bas     # release build plus an unoptimised debug build (blink_debug.*)
```

Which files matter:

| File | Needed? | What the simulator takes from it |
|---|---|---|
| `<name>.hex` | **Yes** | The program image, including configuration words and EEPROM contents |
| `<name>.elf` | Recommended | Written by `basil -g`. Gives routines, the `__halt` address, global variables with their types, each routine's parameters and locals, and the address-to-BASIC-line map. It also names the part it was built for |
| `<name>.S` | Optional | The assembly listing. Gives code labels and variable addresses for `--dump`, `--bp`, `--until` and the Watch tab. If both an ELF and a listing exist, the listing's symbols win |

Rules for finding the files:

* You may give either the `.hex` or the `.elf`. Given an `.elf`, the simulator loads the `.hex` of the same name beside it, or in a `dist/` folder next to it.
* The ELF is looked for beside the hex (`prog.elf`), or one folder up (`../prog.elf`), because basil writes the hex into `dist/` in a project build.
* The listing is looked for as `prog.S` beside the hex (the GUI also tries `prog_basil.S`).

**Debug builds.** Without `-g` there is only the hex (and the listing). You can still run, step instructions, set breakpoints on addresses and watch SFRs. The Source view, line stepping, the Variables tab and routine names in the call stack all need the ELF. In VS Code use *Build Debug* (Ctrl+F7); the plain build (F7) has no line information. `--keep-vars` makes basil keep stores to variables nothing reads, which scripted tests need so that a variable can be dumped at the end.

**Programs from another PIC BASIC compiler.** A compiler that writes an MPLAB-style COFF file (`prog.cof`, for example with `Declare Create_Coff = On`) can be debugged the same way. Give `basil-sim` or `basil-sim-dap` the `.hex`; when there is no `.elf`, the `prog.cof` beside it (or one folder up) is read. The COFF file gives the address-to-line map and the variables with their addresses; the BASIC source it names is read too (where the file records it, else by file name beside the COFF file) to get the routines (`Proc` ... `EndProc`), their parameters, locals and `Result`, and the declared types, because the COFF file writes `Byte` and `SByte` alike. Names are looked up in any case. `Float` is shown in the Microchip variation of IEEE 754 (AN575). The part is taken from the compiler's build log (`prog.pbe`) or the `LIST P =` line of `prog.asm` or `prog.lst` when `--device` is not given. A `Bit` variable is shown as 0 or 1 (the COFF file gives the byte and the bit number); the main program appears in the call stack under the name of its section (`Main`). The `pic-sim` window loads the COFF file beside the HEX the same way (and takes the part from the build log), and the VS Code extension launches such a HEX with no Basil build and no `device`. A `Bit` variable can be set and watched alone through the debug adapter (`setVariable`, data breakpoints): only its bit changes, and only its bit stops a watch.

**The halt address.** basil places a label `__halt` where the program ends. `basil-sim` runs *until* that address by default, and the window stops there and reports it. A program that never ends (a `While True` main loop) never reaches `__halt`; it runs until `--max-cycles` or until you press Pause.

## C programs from XC8

A program built with MPLAB XC8 loads and runs like any other: the simulator executes the `.hex`. Build with `-g` to get the debug information in the `.elf`:

```
xc8-cc -mcpu=18F27Q10 -mdfp=<pack>/xc8 -g -O0 prog.c -o prog.elf
xc8-cc -mcpu=18F27Q10 -mdfp=<pack>/xc8 -g -O0 main.c util.c -o prog.elf    # a program of several files
```

XC8 writes `prog.hex` beside `prog.elf`. Load either one, with `pic-sim` or `basil-sim`, as for a BASIC program (the part is read from the ELF in the window; give `--device` to `basil-sim`). What you get:

| Feature | With `-g -O0` | Notes |
|---|---|---|
| Source pane, breakpoints on a C line, Line / Line over / Out, call stack with lines | Yes | `.c` files (and `.h` files that hold code) are source. The assembly XC8 generates (`prog.s`, `startup.s`) is not shown |
| Routines by name | Yes | `--until`, `--bp`, the call stack. Library routines from XC8's own sources appear too |
| Variables tab, Watch by name | Globals, locals and parameters of integer (up to 64 bits, signed or not), `float`/`double` (XC8 4.00 makes both 32-bit), `_Bool`, `char`, `unsigned char`, array, structure, union, enumeration, bit-field and pointer types, including `volatile`, `const` and `typedef` names (`uint8_t`). A structure shows as `{x = 7 (0x07), y = -300 (0xFED4)}`, an enumeration as `GREEN (5)`, a pointer as its address and, when it points into data memory, the value there (`0x15 -> 42 (0x2A)`; `NULL` for zero). Watch also takes a member or element: `pt.x`, `sh.tag[1]`, `pts[1].y` (add `:bytes` for more than one byte) | At `-O0` XC8 gives each variable a fixed address, which is what the simulator reads. A pointer to its own structure type (`struct node *next`) shows what it points at one level down (`next = 0x15 -> {v = 2, next = NULL}`), so a ring of nodes ends. A pointer into program memory (a `const char *` to a string literal, a pointer to a `const` table) shows the byte it points at too, on a PIC18 and a PIC16 alike. A `const` variable that XC8 keeps in program memory (a lookup table, a table of strings) is read from there and shows its values; Watch takes it too (`tab`, `tab[1]`, add `:bytes` for more) and reads program memory, marking the address `prog`. A function pointer shows its address and the routine it enters (`0x00109A -> a`) |
| Programs of several `.c` files | Yes | Each file's routines, globals and lines come from that file. A `static` variable that two files both define (`static int count;` in each) appears twice in the Variables tab, each named with its file (`count (util.c)`). Two `static` routines of one name are told apart the same way in the call stack. Watch reads the copy belonging to the file of the routine the PC is in, and names that file. Write `name@file.c` (`count@util.c`, `helper@main.c`; the file name without its folder) to pick one copy in Watch, `--until`, `--bp` and `--dump`. `basil-sim` refuses the bare name of such a symbol (section 10). An `extern` declaration adds nothing: the variable is the one its defining file describes (tested at `-O0` on a PIC18 and a PIC16 part) |
| Software stack (`-mstack=reentrant`, `-mstack=hybrid`) | Yes | The parameters and locals XC8 keeps on the stack are read from the frame of each call: the current routine's, and every caller's, one per level of a recursion (tested at `-O0` on a PIC18 and a PIC16 part, reentrant and hybrid, and a tail call at `-O3`). See *Software stack* below |
| Optimised builds (`-O1` and up) | Yes | XC8 4.00 still gives every variable it describes a fixed address, so the Variables tab and Watch read the right values (tested at `-O1`, `-O2`, `-Os` and `-O3` on a PIC18 and a PIC16 part) | The optimiser merges and reorders lines, so stepping jumps about. A local the compiler folds into a constant has no entry at all (at `-O0` too). At `-O2` and above the compiler overlays the locals of routines that are not running, so a routine's locals and parameters are meaningful only while it is on the call stack; a global the compiler folds away (`total` at `-Os`/`-O3`) has no entry. |

**Software stack.** With `-mstack=reentrant` (or for the routines `-mstack=hybrid` puts on the stack, such as a recursive one) XC8 keeps parameters and locals in a frame on a stack that FSR1 points to, and the debug information gives each one as an offset from FSR1. The simulator notes FSR1 at every call and at the first instruction of `main` and of each routine with a stack frame, and adds the bytes the routine's prologue reserves; that is where the call's frame is. What to expect:

* The values are right at every instruction: at a source line, in a routine's prologue and epilogue (the parameters the caller pushed are readable before the prologue runs), and while a statement pushes the arguments of a call (`f(a, g(b))`).
* Each call has its own frame, so the caller sections show each level of a recursion with its own values.
* A tail call: at `-Os` and `-O3` XC8 ends a routine whose last action is a call with a jump to the callee (GOTO, on a PIC16 LJMP) instead of CALL and RETURN, after pushing the callee's arguments. The callee's frame is still right, because FSR1 is noted again at its first instruction. The routine that jumped has no pending return, so it is not on the call stack: the callee returns straight to its caller.
* An interrupt routine gets its own stack: XC8 saves FSR1 and points it at a separate interrupt stack, and keeps the routine's own locals at fixed addresses. Routines it calls show their frames as usual, and below it the caller sections show the interrupted routine (found at the instruction it will resume at) with its frame.
* A pointer into the stack (`&buf[1]`) shows the byte it points at; on a PIC16 it holds a linear address (2000h up).
* The frames are known from the calls the simulator has run since the last Reset. In start-up code, before `main`, there are none.
* Watch takes globals only, as before; use the Variables tab for stack variables.

**Stopping.** A C program has no `__halt`. `basil-sim` then runs for `--max-cycles` and prints a note. Use `--until ROUTINE` to stop when a routine is entered, for example a function your program calls when it is done; unlike basil's `__halt`, a routine entered by a call does stop the run. In the window, Run goes until a breakpoint or Pause.

**Configuration bits** set with `#pragma config` arrive through the hex file, as for BASIC programs. Peripherals that the simulator does not model behave as in section 9 whatever language the program is in.

---

# 4. Supported devices

The parts the simulator can load are the files in `devices/`:

| Family | Parts | Core | Notes |
|---|---|---|---|
| PIC18-Q10 | 18F27Q10 | PIC18 | The 28-pin part; the 40-pin variant is not separately described |
| PIC18-Q41 | 18F04Q41, 18F05Q41, 18F06Q41, 18F14Q41, 18F15Q41, 18F16Q41 | PIC18 | Interrupts with `MVECEN` on (vector table, as XC8 `__interrupt(irq(...))` builds it) or off (see section 9) |
| PIC18-K22 | 18F23K22, 24K22, 25K22, 26K22 (28 pins), 43K22, 44K22, 45K22, 46K22 (40/44 pins) and the **LF** variants 18LF23K22 … 18LF46K22 | PIC18 | No PPS: the peripheral pins are fixed (CCP2 and CCP3 move with Config CCP2MX/CCP3MX); IPEN in RCON; the older Timer0/1/2, ADC and EECON1 memory controller (DS40001412H; `docs/FACTS-SIM.md`, "PIC18(L)F2x/4xK22") |
| PIC16F131xx | 16F13113, 13114, 13115, 13123, 13124, 13125, 13143, 13144, 13145 | PIC16 enhanced | |
| PIC16F132xx | 16F13214, 13215, 13224, 13225, 13244, 13245 | PIC16 enhanced | 8-, 14- and 20-pin parts (DS40002686B); 256 bytes of data EEPROM; the 10-bit ADC converts in 12 TAD + 2 TCY (the PIC16F131xx takes 14 TAD) |
| PIC16F152xx | 16F15213, 15214, 15223, 15224, 15243, 15244 | PIC16 enhanced | |
| PIC16F171xx | 16F17126, 16F17146 | PIC16 enhanced | |
| PIC16F180x6 | 16F18056, 16F18076 | PIC16 enhanced | |
| PIC16F183xx | 16F18313, 18323, 18324, 18325, 18326, 18344, 18345, 18346 and the **LF** variants 16LF18313 … 16LF18346 | PIC16 enhanced (5-bit BSR) | |

The window's device box lists only the parts whose file name starts `18F` or `16F`, so the `16LF…` and `18LF…` parts can be used with `basil-sim --device 16LF18346` but are not offered by `pic-sim`'s box.

Device files are generated from Microchip's device packs by `tools/devgen` and cross-checked against the data sheets; they are never edited by hand (`docs/DEVICE-FORMAT.md`). Each carries the memory map, the special function registers with reset values and read/write masks, the configuration words, the interrupt vectors and the pin list.

---

# 5. The graphical simulator: pic-sim

## The window

```
+--------------------------------------------------------------+
| TOOLBAR: device | hex/elf/cof path | stimulus | Browse | Stim |
|   Load | Run Pause Step ... Reset | Speed | Fosc MHz         |
| status line                                                  |
+----------+----------------------------+----------------------+
| CPU      | Program                    | Memory  SFRs  Watch  |
|  regs    |  Source | Disassembly      |  Board  Variables    |
|  Stop-   |  [x] follow PC             |  UART  Debugger      |
|  watch   |  [x] stay in my code       |  Pins  Peripherals   |
|  Call    |  goto [      ] Go          |  Waves               |
|  stack   |  > 0042  ...               |  (the selected tab)  |
|  Notes   |                            |                      |
+----------+----------------------------+----------------------+
| BOARD STRIP: LEDs and OLED (shown only when wired)           |
+--------------------------------------------------------------+
```

The window title shows `pic-sim <version>`. The window is divided into the **toolbar** and **status line** at the top, the **CPU panel** at the left, the **Program pane** in the middle, the **data tabs** at the right, and the **board strip** along the bottom when LEDs or an OLED are wired.

## Loading a program

1. Pick the part in the device box, or leave it: when the ELF names the part it was built for, that part is selected automatically and the status line says `device set to … from the ELF`. The wrong part would have the wrong registers and pins, so the ELF wins over the box. Choosing another part while a program is loaded loads it again on that part straight away (a run stops), so the box always names the part that is running. (This works only for parts the box lists, so not for the `16LF…` parts: choose those by hand.)
2. Enter a path in the first text field, or press **Browse hex/elf/cof…**, or drag a file onto the window. A `.hex`, `.elf` or `.cof` dropped on the window loads at once (ignored while the program is running). A `.toml` dropped on the window becomes the stimulus file.
3. Optionally enter a stimulus file in the second field, or press **Stim…**. A stimulus file is a TOML file that **you write** (section 7) to feed the program serial input, ADC values, pin levels and so on; none is supplied with your program, and you can leave the field empty to run without one. The **Stim…** dialog opens in the folder of the stimulus file already named, else in the program's folder, else in the repository's `tests/sim` folder, which holds working examples. The small **x** beside the field clears it. **Browse hex/elf/cof…** likewise opens in the program's folder. The stimulus is read when you press **Load**, so choosing one after loading needs another **Load**.
4. Press **Load**.

The status line then reads `loaded <path> (N symbols, M variables)` followed by a note when something is missing: `no .elf beside the hex: no source view or variables`, or `ELF has no source lines (build with basil -g or xc8-cc -g)`. An error (for example a bad stimulus file) is shown there and nothing is loaded.

If the ELF holds variables, the **Variables** tab is selected automatically.

Pressing **Load** again always starts afresh: the program is reloaded, the cycle counter is 0, the stopwatch is cleared, any step in progress is cancelled and the breakpoints are cleared. The wired LEDs and OLED are kept (section 8).

## The toolbar

| Control | Action |
|---|---|
| **Run** | Run the program. Enabled when a program is loaded and nothing is running or stepping. The program runs until it halts, reaches a breakpoint or data watchpoint, goes to Sleep with nothing able to wake it, or you press Pause |
| **Pause** | Stop a run or cancel a step in progress. Enabled while running or stepping |
| **Step** | Execute one instruction |
| **Step over** | Like Step, but a call (PIC18: `CALL`, `RCALL`; PIC16: `CALL`, `CALLW`) runs until it returns to the following instruction. Everything else is one step. A long step over runs in the background, keeps the window alive, and can be cancelled with Pause |
| **Line** | Step to the next BASIC or C source line, going into called routines. Needs the ELF with source lines |
| **Line over** | Step to the next source line, running calls and interrupts to completion |
| **Out** | Run until the current routine returns |
| **Reset** | Restart the part from its reset vector with cycle counter 0 and the stimulus replayed from the start. The wiring of LEDs and OLED stays (the OLED starts blank, as after power-up). Clears the stopwatch, UART terminal history and waveform capture; keeps data watchpoints and trace choices |
| **Speed** | Run speed (below) |
| **Fosc override MHz** | Zero (the default) derives Fosc from the firmware's oscillator configuration and registers. A positive value overrides it, up to 32 MHz on a PIC16 or 64 MHz on a PIC18. The CPU panel shows the effective Fosc. Loading a stimulus with `fosc_mhz` copies that override into the box |

While a step is in progress, **Run**, **Step**, **Step over**, **Line**, **Line over** and **Out** are disabled and only **Pause** is available; the status line shows `stepping... (Pause cancels)`. A source-line step (Line, Line over, Out) also stops early at a breakpoint, a data watchpoint, `__halt`, or Sleep with nothing able to wake the part. Pause cancels a long step.

When a run or step ends the status line shows the reason after the loading message, for example `loaded … — stopped: Halt`. The reasons are:

| Shown | Meaning |
|---|---|
| `Halt` | The program counter reached `__halt` (or the target of a Step over) |
| `Breakpoint(addr)` | A breakpoint address was reached |
| `data watchpoint at 0xADDR: OLD → NEW` | A watched data byte changed; the address and values are hexadecimal |
| `Sleep` | `SLEEP` was executed and nothing that is enabled can wake the part |
| `Cycles` | The cycle budget ran out (`basil-sim` only) |

## Run speed

The **Speed** selector sets how fast simulated time passes against real time:

| Setting | Meaning |
|---|---|
| **Max** | As fast as the host allows. A slider *cycles/frame (10^x)* appears and sets how much is simulated per screen update (10^0 … 10^7; the default is 10^6) |
| **1x (real time)** | The simulated part runs as fast as the real one at the chosen Fosc |
| **1/2, 1/4, 1/10, 1/100, 1/1000** | A fraction of real time |

Use *1x* or a fraction to watch an LED blink or a display scroll at a speed you can see, and Max to get through initialisation. The effective firmware clock (or the Fosc override, when set) determines this calculation: one instruction cycle is `Fosc / 4`, so at 64 MHz real time is 16 million cycles a second. Changing the speed or Fosc while running takes effect immediately without a jump. If the host stalls, the simulator never catches up by more than 0.25 s of real time, so the part does not race through a burst.

While running, the status line shows the achieved rate, for example `running at 100.0% of real time (16.00 Mcycles/s)`. It also reminds you that a wait on a UART or timer flag shows as one source line for a long time — that is the program waiting, not the simulator.

PIC18 GPIO and peripheral outputs also feed the selected timer clocks/gates, capture inputs,
and interrupt-on-change inputs. Internal pin changes propagate on the following instruction
clock; PORT and board displays show the current output. UART TX produces serial bits and RX
can sample the same pin for loopback. PPS, ANSEL, polarity and open-drain settings affect the
resolved signal. Q41 UART outputs retain TRIS control; Q10/K22 EUSART outputs enable their
own driver. MSSP hosts expose SPI and open-drain I2C waveforms, including K22 fixed pins.
SPI receives from its selected input pin when no `[[spi]]` byte client is supplied. I2C clients
remain byte-level devices; external pin-driven I2C clients and clock stretching are not modelled.

## UART terminal

The **UART** tab shows transmitted bytes from the selected modelled UART/EUSART on PIC16 and PIC18 parts. Text displays UTF-8 with other control characters escaped; **Hex** shows the exact byte values. The pane shows at most the latest 16 KiB of output and reports when older output was omitted. **Clear output** hides the selected port's existing output without changing the simulated UART. Loading a program or pressing Reset clears this view's history.

Enter text (sent as UTF-8), or enable **Hex** and enter space-separated two-digit bytes such as `41 00 FF`. Choose no line ending, LF or CRLF, then press **Send**. Up to 4096 bytes including the ending can be queued at once. Input is queued after any existing receiver stimulus and uses the firmware's baud setting and existing timed receiver model; run or step to advance reception. Enable the firmware's receiver before sending. Queued terminal input is transient and is discarded by a reset; stimulus-file input is reapplied as before.

## The Program pane

The pane has two views, chosen with the **Source** / **Disassembly** buttons (the buttons appear only when the ELF supplies source lines).

**Source view.** Shows the BASIC or C file with line numbers. The line the program counter is on is highlighted green, a breakpoint is a red `●`. With **follow PC** on, the view scrolls to keep the current line centred. When the program counter is in generated code that has no source line (start-up code, the compiler's helper routines), the view stays on the last source line and says so. Library routines written in BASIC (`i2c.bas`, for example) show their own file. With **stay in my code** on, a library routine that your code called is shown as the line of your program that made the call, so a `DelaySeconds` appears as one line instead of a jump into the library.

Click a source line to toggle a breakpoint on it; the breakpoint goes on the lowest address of the line. A comment or blank line has no code and cannot take a breakpoint. The compiler's optimiser can merge lines, so some lines have no code.

**Disassembly view.** Shows address, instruction word and mnemonic. The PIC18 listing is in byte addresses; the PIC16 listing is in **word** addresses (the PIC16 program counter counts words). Labels from the listing or ELF appear in blue above their instruction. The current instruction is highlighted; a breakpoint is a red `●`. Click an instruction to toggle a breakpoint at it. The text field *goto 0x…* with **Go** moves the listing to an address (PIC16: a word address, 15 bits) and turns *follow PC* off.

If the build has no source line information, an orange message at the top of the pane says so and tells you how to build with it.

## The CPU panel

The left panel shows the core registers and the cycle counts.

*PIC18:* PC, WREG, BSR, STATUS (with N, OV, Z, DC, C shown as letters when set), FSR0 to FSR2, PROD, TBLPTR, TABLAT, STKPTR.

*PIC16:* PC (word address), WREG, BSR, STATUS (Z, DC, C), PCLATH, INTCON, FSR0, FSR1.

Both show **Cycles** (instruction cycles since reset), **Instr** (instructions executed; some take two or more cycles) and **Time** (elapsed simulated milliseconds, accounting for firmware clock changes or the explicit override).

### Stopwatch

The stopwatch measures the interval between two moments of the *simulated* run, in cycles and in microseconds of simulated time. The time reading accounts for firmware clock changes or the explicit Fosc override, and is independent of the run speed.

| Button | Action |
|---|---|
| **Start** | Start counting from zero, or resume after Stop. The time spent stopped is left out |
| **Stop** | Freeze the reading |
| **Lap** | Record the elapsed time (enabled while counting). Each lap is listed as `Lap n: <total> us (+<since previous lap>)` |
| **Clear** | Zero the stopwatch and the laps |

A typical use: set a breakpoint at the start and end of a routine, press Start when the first is hit, Run to the second, press Stop, and read the time the routine took. A Reset or a Load clears the stopwatch because the counter it was measuring has restarted.

### Call stack

With the ELF, the panel is headed **Call stack**: the first line (`>`) is the routine and BASIC line the program counter is in, followed by the routine and line of each pending return, innermost first (up to 12 on a PIC18, 16 on a PIC16). Without the ELF it is headed **Return stack** and lists the return addresses (on a PIC16 with the nearest label and offset).

### Notes

When the simulator meets something it does not model or that is suspect, it adds a note here, for example `vectored interrupt 27 has no valid vector at 000042 (erased or outside the program memory): the part was reset`, an OLED command with no effect, or `unmodelled SFRs touched: …`. Treat notes as the simulator telling you where the result is not backed by a model.

## The data tabs

### Memory

A hex dump of data memory, 16 bytes a row with an ASCII column. On a PIC16 an address that has no implemented bit shows `--`. Use the Debugger tab to write a data byte. A byte that changed since the last Step or Run began is shown in **orange**, in the hex and in the ASCII column; the comparison is against the snapshot taken when you pressed Run or a step button (the same one the Variables tab uses), so after a Run to a breakpoint the orange bytes are those the run changed. Registers computed by a peripheral, such as PORTx, are compared as they read.

### SFRs

A list of the special function registers with address, hex and binary value, in alphabetical order. The *filter* box matches part of a name (case-insensitive). Reading is without side effects: viewing a register does not clear a flag or pop a receive buffer. While paused, edit the hex value directly; writes use the normal CPU register path, including implemented-bit masks, read-only bits and peripheral side effects. A register that changed since the last Step or Run began is shown in orange (name and binary value), as in the Memory tab; tick **changed only** to list just those registers. Right-click a register's name and choose **Trace NAME in Waves** to add the byte to the capture.

### Watch

Enter `symbol` or `symbol:bytes` and press **Add** to watch a value; `bytes` is 1 to 8 (default 1). The symbol may be a code or data label from the listing, a global variable from the ELF, or an SFR name (`TMR0L`, `PIR0`). Each entry shows address and value as unsigned, signed and hex, for example `T_R1[2] @0123 = 1000 (1000) 0x3E8`. Multi-byte values are little-endian. A `static` global that two C files both define is read from the file of the routine the PC is in, and the entry names that file (`count (util.c)`); `count@util.c` always reads that file's copy. A name that is not found shows `unknown`. Press `x` to remove an entry.

### Debugger

Available for both PIC16 and PIC18. The CPU panel's **Edit…** button opens this tab. Mutation controls are enabled while paused, including between instruction and source steps.

Enter an SFR name, global variable (the same names and `name@file.c` syntax as Watch), or a decimal/`0x` data address. The current byte is shown in hex; choose a new hex value and press **Write**. To change the program counter, enter `PC` (PIC16: word address; PIC18: even byte address). PC values wrap within the part's program memory. Register writes obey the hardware model, including read-only bits and side effects. Multi-byte registers can be edited through their individual byte members. PIC16 linear RAM addresses are translated to banked data addresses.

**Break on value change** adds a data watchpoint on the selected byte. Run, Step, Step over, Line, Line over and Out stop after an instruction changes it; the stop reason reports the address and old/new values. Peripheral changes also count. Reads, same-value writes, and changes that return to the original value within one instruction do not trigger. Edits made while paused become the next step's starting value. **Remove** deletes a watchpoint.

**Break when (conditional breakpoint)** stops the simulator when a data byte satisfies a comparison. Enter an SFR, variable or data address (the same names as above), choose `==`, `!=`, `<`, `<=`, `>`, `>=` and a hex value, or *bit set* / *bit clear* and a bit number, and press **Add condition**. Tick **16-bit** to read the named byte and the next as one little-endian value (`TMR1L`, a 16-bit variable; the comparison and the value then run to 0xFFFF). Two kinds:

* *Without an address*, the condition is **edge-triggered**: the run stops after the instruction that makes it true, not on every instruction while it stays true, so `PORTB bit 2 set` stops when the pin goes high, and a condition that already holds when you press Run does not stop the run until it has become false and true again. A change made by a peripheral between instructions counts, as it does for **Break on value change**.
* With an address in **at** (hex; the **PC** button fills in the current program counter; a PIC18 byte address, a PIC16 word address) the condition is a **conditional code breakpoint**: the simulator stops *before* that instruction runs, whenever the comparison holds at that moment.

**hit** makes it stop on that hit and every later one (`hit 3`: ignore the first two); the count starts again at Reset. Each condition is listed with its current value and hit count and turns orange while it holds. Run, Step over, Line, Line over and Out all stop on a condition, and the status line says which one. Conditions are kept until removed (Load clears them) and, like watchpoints, make the simulator check after every instruction, so a run with conditions is slower. They are not available in `basil-sim` or the VS Code debug adapter, which keep their own breakpoints.

**External inputs:** enter an existing pin such as `RA0`, select **High**, and press **Drive pin** to drive high or low at the current cycle. Enter a decimal ADC channel and conversion code and press **Apply** to change the ADC input. Codes use the part's ADC resolution. These changes use the stimulus model, persist until overridden by a later stimulus/input change, and are cleared by Reset or Load. Pin direction and analog selection still apply.

**Waveform capture:** select a data byte and press **Trace data byte**, or select a pin above and press **Trace pin**. Choose signals before recording; **Clear capture** allows changing the signal list again. **Record** captures an initial sample at the next step and subsequent values at instruction boundaries while stepping or running. **Stop capture** pauses recording; **Record** resumes on the same simulated timeline. The capture is drawn in the **Waves** tab; export retains every stored sample.

**Export VCD…** saves the capture with a 1 ns timescale and timestamps relative to its first sample, using simulated time (including clock changes). Pin traces are the digital PORT readings, so ANSEL applies. Pulses entirely within an instruction may be missed. Capture stops at 100,000 stored samples and reports that it is full; unchanged intervals are compacted while preserving their duration. Reset clears the capture and keeps watchpoint/signal choices; Load starts with a new debugger configuration.

### Pins

Every pin of the part at the current cycle, one row per port and one cell per bit (bit 7 at the left). A cell shows the electrical state:

| Cell | Meaning |
|---|---|
| `H` / `L` | the part drives the pin high / low (a TRIS output or a peripheral that owns the pin) |
| `1` / `0` | an input reading high / low (a floating input reads low unless a stimulus, a pull-up or the board drives it) |
| `A` | analog: ANSEL selects the analog function, so the digital input is off and the pin reads 0 |

Right-click a pin and choose **Trace RBn in Waves** to add its PORT bit to the capture (the same as **Trace pin** in the Debugger tab). A cell with an **orange outline** has a different level from when the last Step or Run began. Hover a cell for the pin name and state. While the simulator is stopped, click an input cell to drive it to the opposite level; this is the same input as the Debugger tab's **Drive pin** (it persists until overridden, Reset or Load). An open-drain output that is releasing the pin shows as an input. The view reads the pins without side effects and, as everywhere in `pic-sim`, at instruction boundaries.

### Peripherals

The state of the timers, the EUSART, the ADC and the interrupt flags, decoded from the special function registers (there is no separate model state: it is what the registers hold, so it is exactly what the program can read). Reading is without side effects.

* **Interrupts**: the global enables (`GIE`, `GIEH`/`GIEL`, `PEIE`, `IPEN`, where the part has them) and every interrupt flag (`xxxIF`) that is set, with its enable (`xxxIE`). A flag whose enable is set is checked against the global enables:
  * *will interrupt* (orange): nothing blocks it; with `IPEN` = 1 the priority is named (from the flag's `xxxIP` bit; a source with no priority bit, such as INT0 on the K22, is high priority);
  * *enabled, blocked by …* (red): the clear global enables that block it, for example `GIE=0` or `PEIE=0`;
  * *flag only*: the enable is clear (or the flag has no enable bit of that name).

  The rules follow the data sheets (Q10 DS40002043F 15.13.1 p.198; Q41 DS40002214G 11.13.1 p.127; K22 DS40001412H Register 9-1 p.109; PIC16F17126/46 DS40002343F 12.10.1-2 p.114-115, and the same note in the F131xx/F132xx/F152xx/F180x6 data sheets). With `IPEN` = 0, `GIE` enables everything and `PEIE` also gates the sources of PIE1 and up (not PIE0, and not the INTCON sources); the Q41 has no `PEIE`. With `IPEN` = 1, a high-priority source needs `GIEH` and a low-priority one needs `GIEH` and `GIEL`. The PIC16F183xx data sheet says `PEIE` must be set for "any peripheral interrupt" without naming PIE0; the simulator treats PIE0 as not gated, like the other PIC16 families. The view says whether the interrupt *can* be taken: it does not know about an interrupt already in service (the Q41 and the PIC18 priority logic hold off an interrupt of the same or lower priority).
* **Fire an interrupt**: below the list, every interrupt flag the part has (filter by name), with its value and a **Fire** button (a flag that is clear) or **Clear** button (a flag that is set). **Fire** sets the flag bit as an event would, and the interrupt is taken on the next Step if the gating above allows it. Tick **also enable it** to have **Fire** also set the flag's enable bit and whichever of `PEIE`, `GIE`/`GIEH` and `GIEL` block it (with `IPEN` = 1 and a low-priority source both `GIEH` and `GIEL`); the enables are written first and the flag last. The status line reports the result: *will interrupt*, *blocked by …*, *its enable bit is clear*, or *did not set: the bit is read-only or hardware-controlled* (for example the Q41 UART error flags). The buttons work only while the simulator is stopped. A written flag is not a real event: the peripheral does not see it, so the flag can be cleared by the program or by **Clear** but nothing sets the peripheral's own status. A realistic interrupt comes from the peripheral itself, from a pin edge (Pins tab, **Drive pin**) or from a stimulus file. On the Q10 the simulator lets a write set `IOCIF` although the data sheet calls it read-only (DS40002043F p.199, note 2); use a pin edge for a faithful IOC interrupt.
* **Timers**: for each `TMRn` the count in hex and decimal (a 16-bit timer is read as one value), its period register `PRn` where it has one, and the timer's control registers (`TnCON`, `TnCLK`, `TnGCON` ...) with the bits that are set.
* **EUSART** and **ADC**: the control, status and baud/result registers with their set bits; the baud-rate generator and ADC result are joined into one value.

A register that changed since the last Step or Run began is orange. For a bit field, `NAME=3` is its value in hex; a bit field that is zero is not listed.

### Waves

The waveform captured for the signals chosen in the Debugger tab, or by right-clicking a pin in Pins or a register in SFRs. The signal list can change only while there is no capture (press **Clear capture** first; the menu item is disabled otherwise). **Record**, **Stop capture**, **Clear capture** and **Export VCD…** are repeated here. Each signal has a lane under a time axis: a pin is a high/low trace, a data byte is a bus with its hex value. Scroll over the plot to zoom about the pointer, drag to pan, or use **Zoom in**, **Zoom out** and **Fit**; hover to read the time and every signal's value at that point. Every transition is drawn unless the window holds more than about 4,000 samples, when the plot is thinned for speed (zoom in to see the detail; the export always keeps every sample; a capture is at most 100,000 samples). The Waves tab shows the same samples the VCD export writes, so nothing needs an external viewer for a first look.

### Board

Wire LEDs and an OLED display to pins (section 8). Available even before a program is loaded.

### Variables

The globals of the program and the parameters and locals of the routine the program counter is currently in, with type, data address and value:

* `Integer` is shown signed, `Char`/`String` as text, `Float` as a binary32 number, arrays element by element.
* A parameter is tagged `(arg)`.
* Below the current routine, each routine with a pending call on the call stack gets its own **caller: <routine>** section with its parameters and locals. A routine whose variables have fixed addresses gets one section, even when it recurses, because every call of it shows the same bytes. A routine with variables on XC8's software stack (section 3, *Software stack*) gets one section per call, so each level of a recursion shows its own values. The values are read from memory as they are now; a caller's variable the optimiser has merged or reused may not hold what the source suggests.
* A variable on the software stack shows its address in the call's frame (`@07F`; on a PIC16 a linear address such as `@2027`). Where the frame is not known (no call noted, see below) it shows its offset from the frame base (`@frame-8`: FSR1 at the call plus what the routine's prologue reserves) and the value `(no frame here)`.
* The heading says `in <routine>` or `in start-up code (globals only)`.
* A value that changed since the last Step or Run began is shown in **orange**. The comparison is against a snapshot taken when you pressed Run or a step button, so after a Run to a breakpoint the orange variables are those that the run changed.
* The changed-since-last-step marker on a pointer covers the pointer variable's own bytes. If the pointed-to value changes, the row is not highlighted.

The values are live while running. Without the ELF the tab only says that variable information needs `basil -g` and the ELF beside the hex.

## The board strip

When LEDs or an OLED are wired, a strip along the bottom shows them. An LED is a disc whose brightness follows the fraction of time the pin was at the active level over the last window of time, so a PWM-dimmed or multiplexed LED shows an intermediate brightness. An OLED is drawn at three times its size in light blue on black; if the OLED is not answering (wrong pins, wrong address, missing PPS set-up) its caption says `NOT WIRED:` and why.

## Typical sessions

**Find why a variable has the wrong value.** Build with `basil -g`, load, click the source line where the variable is set, Run to it, then Line / Line over and watch the Variables tab for orange entries.

**Time a routine.** Breakpoints at its first and last line, Stopwatch as above.

**Check an output waveform.** Right-click a pin in the Pins tab (or a register in SFRs) and choose **Trace … in Waves**, open Waves, press **Record** and Run or Step. Pause and inspect the plot: scroll to zoom, drag to pan, hover to read values. Use **Export VCD…** to open the capture in another viewer. For visual feedback, wire an LED and run at 1/10 or 1/100 speed; `basil-sim` also reports LED changes (section 6).

**See what a program changed.** Press Run (or a step button), pause, and look for orange in Memory, SFRs, Pins and Peripherals: everything that changed since the run began. Tick **changed only** in SFRs to list just those registers.

**Stop when a register or pin reaches a value.** In Debugger, under **Break when**, enter `TMR1` (tick **16-bit**), choose `>=` and a value, press **Add condition** and Run. For a pin, use `PORTB` with *bit set* and the bit number. Add an address in **at** to stop only at one line of code, and **hit** to skip the first hits.

**Exercise an interrupt handler.** In Peripherals, find the flag in **Fire an interrupt**, tick **also enable it** and press **Fire**, then Step: the status line says whether the interrupt will be taken, and the call stack shows the handler. To see the real hardware path instead, drive the pin in the Pins tab or use a stimulus file (section 7).

**Test serial input.** Write a stimulus file with a `[[uart]]` section, load it with the program, and Run; the characters arrive at the receiver at the cycle you chose. Watch the receive variables in the Variables tab.

---

# 6. The command-line simulator: basil-sim

## Synopsis

```
basil-sim prog.hex|prog.elf [--device PART] [--devices DIR]
          [--asm prog.S] [--until SYMBOL] [--max-cycles N]
          [--stim FILE] [--dump NAME[:BYTES],…]
          [--bp SYMBOL,…] [--bp-max N]
          [--trace FROM-TO] [--trace-regs NAME,…] [--config]
          [--speed max|RATIO] [--fosc MHZ]
basil-sim --help | --version
```

The program runs from reset until it stops, then `basil-sim` prints a report and exits.

**PIC16 parts report less.** The PIC16 path prints the stop line, `--bp` lines (without the timer values), `--dump` values, `tx` text and the notes. It does not print the `spi`, `i2c`, `pins`, `led` and `oled` lines, and `--config` is refused (`--config is not supported for PIC16 parts yet`). For LEDs and an OLED on a PIC16 part use `pic-sim`, which draws them. The descriptions below say *(PIC18 only)* where this applies.

## Options

| Option | Default | Meaning |
|---|---|---|
| `prog.hex` / `prog.elf` | required | The program. Given an `.elf`, the hex beside it (or in `dist/`) is loaded; exactly one may be given |
| `--device PART` | `18F27Q10` | The part, as the name of a file in `devices/` without `.toml` (`18F16Q41`, `16F17146`, `16LF18346`). The part is **not** taken from the ELF here, so give the right one |
| `--devices DIR` | see section 2 | Folder holding the device files (else `PICSIM_DEVICES`, `devices/` beside the executable, `../share/picsim/devices`, then the workspace) |
| `--asm FILE` | `<hex>.S` | The assembly listing to take symbols from |
| `--until SYMBOL` | `__halt` | Stop when the program counter reaches this symbol, which must exist in the listing or ELF or the run does not start. A routine's name stops the run when the routine is **entered**, including by a call (useful for C). When no `--until` is given and the program has no `__halt` (a C program), the run is not stopped by a symbol: it lasts `--max-cycles`, with a note on standard error |
| `--max-cycles N` | 2 000 000 000 | Stop after this many instruction cycles even if the symbol was not reached |
| `--stim FILE` | none | Stimulus file (section 7) |
| `--dump LIST` | none | After the run, print these symbols, SFRs or data addresses (`0x20` in hex, `32` in decimal; an address outside data memory reads 0); `NAME` is one byte, `NAME:N` is *N* bytes read little-endian (*N* from 1 to 65536; any other length is refused before the run) |
| `--bp LIST` | none | Comma-separated code symbols. Each time the program reaches one, a line is printed and the run continues |
| `--bp-max N` | 20 | Stop after this many breakpoint hits |
| `--trace FROM-TO` | off | Run silently to cycle FROM, then print every instruction until cycle TO, then exit; FROM after TO is refused before the run |
| `--trace-regs NAME,…` | see below | The registers each `--trace` line shows, by data-sheet name; a name the part does not have is an error before the run |
| `--config` | off | Print the configuration words and the oscillator and watchdog registers they set (PIC18 only) |
| `--speed max\|R` | `max` | Limit the run to a fraction of real time: `1` is real time, `0.5`, `1/10`, `0.001` … |
| `--fosc MHZ` | Firmware | Override Fosc for simulation timing and pacing; at most 32 MHz on a PIC16 or 64 MHz on a PIC18 |
| `--strict` | off | Stop after the first instruction that accesses an unmodelled SFR; report its name and exit with status 3 (also applies to tracing) |
| `--help`, `-h` | | Print the synopsis and a line for each option, and exit |
| `--version`, `-V` | | Print `basil-sim <version>` and exit |

Any other argument that starts with `-` is refused as an unknown option, so a program path cannot start with `-` (write `./-prog.hex`).

In a C program of several files, a symbol given to `--until`, `--dump` or `--bp` can carry its file: `helper@main.c`, `count@util.c:2`. That is required when two files each define a `static` of that name; the bare name is then refused before the run.

An unknown option, a missing value, an unreadable file, an unknown device, a bad stimulus file or an unknown symbol stops with `basil-sim: <message>` on standard error and exit status 2.

## Exit status

| Status | Meaning |
|---|---|
| 0 | The program reached `--until` (by default `__halt`), or no stop symbol applied (a C program without `--until`) and the run completed |
| 1 | The run stopped for another reason: cycle limit, breakpoint limit, Sleep |
| 2 | An error (see above); nothing was simulated |
| 3 | Strict mode encountered an unmodelled SFR |

Strict mode is opt-in and checks unmodelled SFR accesses, not every modelling assumption or warning. Without it, these accesses continue to produce notes. For example, use `basil-sim program.hex --device 18F16Q41 --strict` to reject an unsupported peripheral access.

This makes `basil-sim` usable in scripts: a test program that ends at `__halt` returns 0, one that hangs hits `--max-cycles` and returns 1.

## The report

After the run, `basil-sim` prints these lines, each only when it has something to show.

**Stop line (always).**

```
stop: Halt  cycles: 30414  instructions: 20118
```

The reason is `Halt`, `Cycles`, `Sleep` or `Breakpoint(address)`, followed by the instruction cycles and instructions executed.

**Breakpoint lines.** One for each hit of a `--bp` symbol, before the stop line:

```
bp 1 T_LOOP cycles 1204 tos 0x0018 TMR0L 17 TMR0H 0
```

On a PIC18 the line also shows `TMR0L`/`TMR0H`; `tos` is the top of the return stack.

**Dumps.** For `--dump T_R1:2,TMR0L`:

```
T_R1 = 1000  (E8 03)
TMR0L = 17  (11)
```

The decimal value is the little-endian combination of the bytes, followed by the bytes in memory order.

**Serial.** `tx1: …`, `tx2: …` and so on, one line per EUSART/UART that sent anything (UART3 on the Q41 parts included), show what it transmitted, as a string with control characters escaped (`\r\n`).

**SPI (PIC18 only).** `spi1: 9F 00 00` lists the bytes the program sent through SPI 1, in hex.

**I2C (PIC18 only).** `i2c1: S 6FW! P` is the bus activity of the hardware I2C host: `S` start, `Sr` repeated start, `P` stop, an address as `<7-bit hex>` followed by `R` or `W`, data bytes in hex, and a `!` after an address or written byte that nobody acknowledged, or after a byte the host answered with NACK (the last byte of a read). The example is a write to address 6F that no device answered. A complete transaction with a client at 50 looks like `S 50W 10 Sr 50R CA FE! P`.

**Pin log (PIC18 only).** `pins: RC3=1@4 RC3=0@96 …` lists changes of pins that a peripheral drives (PPS outputs: PWM, serial TX, SPI, CWG …), as `R<port><bit>=<level>@<time>`. The time is in **oscillator ticks**, four per instruction cycle.

**LEDs (PIC18 only).** One line for each LED in the stimulus file:

```
led heartbeat (RB0, active high): ON, on 11999995 of 20000001 cycles, 3 changes: 1@2 0@8000004 1@16000008
led error (RB1, active low): off, on 8000006 of 20000001 cycles, 4 changes: 1@1 0@3 1@8000005 0@16000009
```

It gives the final state, the cycles lit out of the total, the number of changes and the first 40 changes as `level@cycle`.

**OLED (PIC18 only).** A line `oled1 SSD1306 0x3c on SCL RC3 SDA RC4 (I2C1): display on, 1030 data bytes`, followed, if the display is on, by the picture drawn as text. If the display did not answer, `NOT WIRED: <reason>` is appended.

**Notes (standard error).** `note: …` lines for things the model flags, and finally `note: unmodelled SFRs touched: …`, the registers of unmodelled peripherals that the program used. Notes go to standard error, so the report on standard output stays clean for scripts.

## Examples

Run a program to its halt and print two variables:

```
basil-sim dist/test.hex --device 18F16Q41 --dump T_R1:2,T_R2
```

Run with serial input and check the transmit text:

```
basil-sim serial_rx.hex --device 18F27Q10 --stim serial_rx.stim.toml --dump SRX_B0,SRX_FERR
```

Blink program for one second of simulated time, with LEDs described in the stimulus file (`--until` is given a symbol that does not occur, so the run is limited by `--max-cycles`: 16 000 000 cycles is one second at 64 MHz):

```
basil-sim tests/sim/blink_q10.hex --device 18F27Q10 --stim tests/sim/blink.stim.toml --max-cycles 16000000
```

Measure cycles between two routines:

```
basil-sim prog.hex --device 18F16Q41 --bp T_START,T_END --bp-max 2
```

The two `bp` lines give the cycle counts; subtract them. Divide by `Fosc / 4` for time.

Run paced at 1/10 of real time on a 32 MHz clock (the report is still printed only at the end; for live feedback use `pic-sim`):

```
basil-sim prog.hex --speed 1/10 --fosc 32 --max-cycles 3200000
```

Print the instructions between cycles 1000 and 1040:

```
basil-sim prog.hex --trace 1000-1040
```

On a PIC18 each trace line shows the cycle, address, mnemonic, the cycles the instruction took (`+n`) and the value of each register in hex. By default a PIC18 shows `TMR0L`, `PIR0` and `INTCON` (or `INTCON0` on the parts that have it, such as the Q41), and a PIC16 shows `WREG` and `STATUS`. `--trace-regs` replaces that list:

```
basil-sim prog.hex --trace 1000-1040 --trace-regs WREG,LATA,PIR3
```

---

# 7. The stimulus file

A stimulus file is a TOML file that describes everything the outside world does to the part. It is given with `--stim FILE` or the *Stim…* field. Each section is written as `[[name]]` and may be repeated. **Time** is measured from reset, written `at`; integer values count instruction cycles and strings may carry time units (below). An entry with `at = 0` (the default) applies from the start. Where several entries set the same thing, **the later one replaces the earlier from its `at` on**.

An unknown section or key is an error, not ignored, so a misspelled name cannot silently do nothing. The largest value of `at`, `bit` and `gap` is 2^32.

A stimulus file is replayed from the start whenever the part is reset.

## Time units

Every `at`, UART `bit`/`gap`, bounce `period` (including range endpoints), clock `period`/`high`/`until`, and UART-device `delay` accepts an integer cycle count or a string such as `"100 cycles"`, `"250ns"`, `"1.5 us"`, `"20ms"`, or `"0.1s"`. Physical units require a positive top-level `fosc_mhz`, before any tables:

```toml
fosc_mhz = 16
[[uart]]
at = "1ms"          # 4000 instruction cycles at 16 MHz
bytes = "Hello"
bit = "104.167us"
```

Times convert once when the file loads, using one instruction cycle = four oscillator clocks, rounded up to the next whole cycle. Firmware oscillator changes and the GUI/CLI clock setting do not retime these entries. Values must be non-negative, finite, and at most 2^32 cycles after conversion. Bare integers keep their existing meaning.

## Pins: `[[pin]]` and `[[port]]`

Drive the level of input pins.

```toml
[[pin]]            # one pin
port = "B"         # letter A..H
bit = 5            # 0..7
level = 1          # 0 or 1 (or true/false)
at = 0

[[port]]           # several pins at once
port = "B"
value = 0x21       # levels of the pins in mask
mask = 0xFF        # default 0xFF
at = 20000
```

An input pin that nobody drives reads **0**. A pin shows the driven level only when it is an input (`TRISx` bit set) **and** digital (`ANSELx` bit clear); an output pin shows its latch.

## Periodic clocks: `[[clock]]`

```toml
[[clock]]
pin = "RC0"
at = 1000         # first rising edge; default 0
period = 200      # rising-edge interval, at least 2 cycles
high = 50         # high duration; default floor(period / 2)
until = 10000     # optional exclusive end; omit to run indefinitely
```

The pin goes high at `at`, low `high` cycles later, and repeats every `period`. `high` must be positive and less than `period`; `until` must be later than `at`. After `until`, the pin holds the level of its final edge. There is one clock per pin. Clocks generate edges as needed, so an indefinite clock does not expand into a stored list.

Clocks use the same input path as `[[pin]]`, including interrupts, external timer clocks, gates and Sleep wake-up. A static pin/port/bounce change takes precedence at the same cycle; the next clock edge resumes the waveform. The pin must exist on the selected device.

## Contact bounce: `[[bounce]]`

A switch or relay contact that chatters before it settles. The entry is a train of ordinary pin changes, so everything that sees `[[pin]]` changes (PORTx, interrupt-on-change, the INT pins, timer clock and gate pins, capture, CLC and CLB inputs) sees every edge of the chatter.

```toml
[[bounce]]
pin = "RB0"        # the PIC pin
level = 0          # the level the pin goes to at `at` and settles at (0, 1, true, false)
at = 20000         # the first change, default 0
bounces = 3        # times the contact springs back before it settles, 0..1000
period = 150       # cycles between changes: one number (1..2^32), or a range
# period = [100, 400]   # each interval drawn from 100..400
# seed = 1              # with a range only: which intervals (default 1)
```

The pin changes `1 + 2 x bounces` times: to `level` at `at`, then to the other level and back, `bounces` times, one interval apart; the last change leaves it at `level`. With `bounces = 3, period = 150, at = 20000, level = 0` the pin reads 0 from 20000, 1 from 20150, 0 from 20300, 1 from 20450, 0 from 20600, 1 from 20750 and 0 from 20900 on. A press and a release are two entries.

With a range the intervals are pseudo-random but fixed by `seed`: the same file gives the same train on every run and after every reset, and another seed gives another train. The simulator has no electrical model of a contact: the level is a clean 0 or 1 at every cycle, with no slow edge or voltage between the thresholds. The times are yours to choose; take them from the switch's data sheet or a scope capture.

Where a `[[pin]]` or `[[port]]` entry changes the same pin at the same cycle as a bounce, the `[[pin]]`/`[[port]]` level wins. Two bounces on one pin must not overlap in time (start the release after the last change of the press), a train that would end after cycle 2^32 is an error, and so is a pin the part does not have.

## Serial input: `[[uart]]`

Characters sent to the receiver of an EUSART (PIC18-Q10, all PIC16) or UART (PIC18-Q41).

```toml
[[uart]]
port = 1             # EUSART/UART number, default 1
at = 20000           # cycle at which the first Start bit falls, default 0
bytes = "Hello\r"    # a string, or:  data = [0x48, 0x165]
bit = 139            # the sender's bit time in instruction cycles; default: the receiver's own rate
gap = 0              # idle cycles between characters, default 0
nine = false         # 11-bit characters: data values up to 0x1FF, bit 8 is the ninth bit
bad_stop = false     # send the Stop bit as 0 (a framing error)
```

Give exactly one of `bytes` and `data`. Each character of `bytes` is one byte, so it must be U+0000 to U+00FF (Latin-1); any other is an error, and byte values go in `data`. A character whose Start bit falls while the receiver is off is lost, as on the part. The receiver's FIFO, framing and overrun errors behave as the data sheet describes (`docs/FACTS-SIM.md`).

## Reactive serial devices: `[[uart_device]]`

```toml
[[uart_device]]
port = 1             # default 1
request = "AT\r"      # required nonempty string or array of bytes
reply = "OK\r\n"      # required string or byte array
delay = 100          # cycles after the final transmitted Stop bit; default 0
bit = 139            # optional sender bit time; defaults to receiver rate
gap = 0              # idle cycles between reply characters
```

Each rule watches completed transmitted bytes and sends its reply whenever the request matches the most recent bytes. Rules repeat, allow overlapping matches, and can share a port. Strings use the same Latin-1 byte mapping as `[[uart]]`. Matching starts only after the request's final Stop bit; a transmission aborted before completion cannot trigger a reply. Replies go through the timed receiver, with its normal enable, FIFO and overrun behavior. They are eight-bit frames with a valid Stop bit.

Devices work with PIC16/PIC18 EUSARTs and Q41 UARTs. Scheduled `[[uart]]` input and replies share one receive line; queued frames serialize when they overlap. An explicit simulator Reset recreates the rules and their match history.

## ADC input: `[[adc]]`

The conversion result for a channel.

```toml
[[adc]]
channel = 16     # the value written to ADPCH (or the channel number of the simple ADC)
value = 0xABC    # the code the conversion produces, 0 .. 2^bits - 1
at = 600
```

The simulator does not model voltages here; it returns the code you give (limited to the converter's resolution and justified as the ADC's own settings require) for the channel selected when the acquisition ends. A channel with no entry, or one whose first entry is still in the future, converts as 0.

## Analogue pin voltage: `[[analog]]`

The voltage on a pin, for the comparators (PIC18-Q10 and PIC18-Q41).

```toml
[[analog]]
pin = "RA0"
volts = 1.5      # 0 .. 6
at = 0
```

## I2C client: `[[i2c]]`

A device on the I2C bus of a hardware host (PIC18 only: Q10 MSSP, Q41 I2C).

```toml
[[i2c]]
port = 1             # MSSP / I2C module number, default 1
address = 0x50       # 7-bit address (required)
data = [0xCA, 0xFE]  # bytes returned to the host's reads, in order
fill = 0xFF          # returned after data is used up, default 0xFF
```

The client acknowledges its address and every byte written to it, and answers reads from `data`. Several `[[i2c]]` entries make several clients; two at one address on one port are an error. A host that addresses an unlisted device gets a NACK (shown as `!` in the `i2c` report), and a read from nobody returns `0xFF`. The bus is assumed to have pull-ups.

For a reactive register device, add `mode = "memory"`:

```toml
[[i2c]]
address = 0x50
mode = "memory"
data = [0x12, 0x34]  # initial registers starting at 0; at most 256 bytes
fill = 0xFF          # initial value for the remaining registers
readonly = false    # optional; true acknowledges writes but preserves register contents
```

The device has 256 byte registers and an eight-bit pointer. The first data byte after each write address selects the pointer; subsequent bytes write registers and advance it. Reads return the current register and advance the pointer. It wraps from 255 to 0 and persists across Stop and repeated Start, allowing the usual write-pointer / repeated-start / read transaction. Read-only writes still advance the pointer. An explicit simulator Reset restores the initial contents. The default `mode = "stream"` keeps the original fixed-list behavior; `readonly` requires memory mode. These devices are byte-level models without clock stretching or EEPROM write delays.

## I2C host: `[[i2c_host]]`

An external host that talks to the MSSP while it runs in I2C client mode (`SSPM` = `0110` or `1110`, 7-bit address; `0111` or `1111`, 10-bit address: the host sends `w 0xF6` (`11110 A9 A8 0`) then the low byte, the firmware writes SSPxADD after each UA, and a read is a `Sr` with `w 0xF7` after a full match; PIC18 Q10, tested there, and K22, which shares the module). It is byte level for the module, and the bus is shown on the pins: with SCL and SDA selected through PPS (both pins inputs, pulled up by a `[[pin]] level = 1`), PORTx reads the host's Start, bits, acknowledge and Stop as they happen. With SSPEN set the module owns the two pins, so LATx / TRISx do not change them and the module never reads them back. One outside effect is modelled on PIC18 / K22: a `[[pin]]` that drives the SCL pin low holds the host's clock, and the rest of its waveform is delayed by that time (to about an instruction). Data held low on SDA and arbitration are not modelled.

```toml
[[i2c_host]]
port = 1             # MSSP number, default 1
at = 0               # the script starts at this cycle at the earliest, once SSPEN is set
period = 40          # SCL period in instruction cycles, default 40 (at least 2)
ops = ["S", "w 0xA0", "w 0x12", "Sr", "w 0xA1", "r", "rn", "P"]
```

`S`, `Sr` and `P` are Start, Repeated start and Stop. `"w 0xA0"` sends a byte (the address byte included, R/W in bit 0), `"r"` reads a byte and acknowledges it, `"rn"` reads a byte and does not. The run is shown in the `i2c` report like a host's traffic (`!` after a byte the MSSP did not acknowledge). The module honours SSPxADD / SSPxMSK, BF, SSPOV, BOEN, SEN, AHEN / DHEN, ACKDT / ACKTIM / ACKSTAT and CKP stretching: the host waits while CKP is 0. Clearing SSPEN or changing SSPM drops the operation in flight. General call (GCEN) is honoured (`[A167]`). 10-bit addressing, Sleep wake-up and bus arbitration are not modelled. On PIC16 parts the section is accepted at byte level only: the module runs against the host, but the PIC16 pin model does not put the MSSP waveforms on the pins. On PIC18 and K22 an outside device (a `[[pin]]` driving the pin low) holding SCL low stops the host's clock (`[A165]`), and one holding SDA low clears the data bits it covers at the rising SCL edge and turns a host NACK into an acknowledge (`[A166]`; no Start/Stop from it, no arbitration). On PIC16 the pin model is not fed back: an outside device holding SCL or SDA low changes nothing there.

## SPI client: `[[spi]]`

The device at the other end of an SPI host (PIC18 only).

```toml
[[spi]]
port = 1               # SPI / MSSP number, default 1
data = [0xEF, 0x40]    # the byte returned for each byte the host exchanges, in order
fill = 0               # after data is used up, default 0
```

## CLB netlist: `[clb]`

The PIC16F131xx and PIC16F132xx have a Configurable Logic Block. On the part its logic is set by a bitstream from the Microchip CLB Synthesizer that the NVM scanner loads from program memory; the layout of that bitstream is not published, so the simulator takes the netlist from the stimulus file. The numbers are the data sheet's (DS40002686B ch.29, DS40002519D ch.29). A part without a CLB rejects the section; a program that enables the CLB without one gets a note and every BLE output stays low.

```toml
[clb]
divider = 8                 # BLE_clk = CLBCLK / divider (default 1)
output = [0, 2]             # CLBPPSOUTn is BLE[4n + this], 0..3 (default 0)
irq = [0, -1, 5]            # CLB1IF0 on a rising edge of BLE[0]; -1 for none; up to 4 entries
[clb.counter]               # the 3-bit counter (default: off)
enable = true
stop = 3                    # BLE whose output, high, holds the count
reset = 4                   # BLE whose output, high, clears it
a1 = 3                      # Counter[A1] is high while the count is 3 (a1 a2 b1 b2 c1 c2 d1 d2)
[[clb.input]]               # one of the 16 input selections
n = 0
source = 0                  # Table 29-1 value: 0..3 = CLBIN0-3PPS pins
edge = "pos"                # "none" (default), "pos" or "neg"
sync = true                 # synchronised to BLE_clk
[[clb.ble]]                 # one of the 32 BLEs
n = 1
a = 8                       # LUT inputs A..D: 5-bit selections (BLE 0-7 of the group, 8-11 synchronised
b = 31                      #   input, 12-19 CLBSWIN bit, 20-21 counter); default 31, nothing (reads 0)
lut = 0xAAAA                # bit A + 2B + 4C + 8D is the output
flop = true                 # the output is the flop, not the LUT
```

Pins read as the `[[pin]]` entries drive them (set `CLBINnPPS` and clear `ANSELx`). The model takes CLBCLK = FOSC and the CLBIN/CLBCLK pins; the other clock sources and Table 29-1 inputs are not modelled and a note names them. Assumptions: `docs/FACTS-SIM.md` `[A133]`-`[A140]`.

## Reset pin: `[[mclr]]`

Hold the part in reset from a given time (PIC18 only).

```toml
[[mclr]]
level = 0        # 0 holds the part in reset when the pin is configured as MCLR
at = 20000
[[mclr]]
level = 1
at = 40000
```

The pin is high until driven.

## Clock for the display: `fosc_mhz`

```toml
fosc_mhz = 64     # the PIC clock: 0 < MHz ≤ 32 on a PIC16, ≤ 64 on a PIC18
```

An optional top-level key. It gives the clock used for the **OLED's scrolling** time (the OLED's own timing is in real time, the program's in cycles), the watchdog period and the timers' LFINTOSC/SOSC clocks. It applies with or without `[[led]]` and `[[oled]]` entries. Default: derive the nominal frequency from firmware oscillator settings (`RSTOSC`, `OSCFRQ`, clock source and divider; K22 `FOSC`, `OSCCON.IRCF`, `SCS` and `OSCTUNE.PLLEN`). Explicit positive values override firmware; zero restores automatic timing. External or reserved selections whose frequency cannot be derived use the family maximum (32 MHz PIC16, 64 MHz PIC18) and add a note. A value above the part's maximum is refused.

Firmware clock changes update watchdog and LFINTOSC/SOSC timer timing, OLED scrolling, run-speed pacing, elapsed time and the stopwatch. On the PIC18F27Q10 and PIC18F16Q41 family and on the PIC16 parts with `OSCCON1` (PIC16F171xx, F131xx/F132xx, F183xx) a clock switch takes the oscillator start-up time (1024 Tosc for a crystal, 200 µs for the PLL) with the old clock running meanwhile, then sets `NOSCR`, `ORDY` and `CSWIF`; `CSWHOLD` holds it. On the PIC18(L)F2x/4xK22 a change of `SCS`, of the internal oscillator enable or of the PLL keeps the old clock and the old `OSTS`/`HFIOFS`/`PLLRDY` for the crystal OST (1024 Tosc), the 2 ms PLL lock or the 0.25 ms HFINTOSC stabilisation. Parts without a run-time switch (PIC16F152xx, PIC16F18056/76) have nothing to time. `CSWEN` and `CSWHOLD` are honoured on parts with source switching. Oscillator trim and fail-safe clock monitoring are not modelled. EEPROM/flash operations already in progress keep the completion cycle calculated at their start. The CLI's `--fosc` overrides both simulation timing and pacing, and takes precedence over a stimulus clock.

## Board parts: `[[led]]` and `[[oled]]`

See section 8.

## What each family accepts

| Section | PIC18 Q10 | PIC18 Q41 | PIC18 K22 | PIC16 |
|---|---|---|---|---|
| `[[pin]]`, `[[port]]`, `[[bounce]]`, `[[clock]]` | yes | yes | yes | yes |
| `[[uart]]`, `[[uart_device]]` | yes | yes | yes | yes |
| `[[adc]]` | yes | yes | yes (`channel` is the AN number, the CHS value) | yes |
| `[[led]]`, `[[oled]]` | yes | yes | yes | yes |
| `[[i2c]]`, `[[spi]]` | yes | yes | yes | **no** |
| `[[analog]]` (comparators) | yes | yes | — | **no** |
| `[[mclr]]` | yes | yes | yes | **no** |
| `[clb]` | **no** | **no** | **no** | PIC16F131xx and PIC16F132xx only (an error on any other part) |

On a PIC16 part a section marked *no* is an **error** (`[[spi]] is not modelled on PIC16 parts yet…`) rather than being ignored. A PIC16 stimulus is also an error if it names a pin or port the part lacks, an EUSART it does not have, or an OLED on an I2C module the simulator does not model for that part (use `bus = "gpio"` instead).

## Examples from the repository

`tests/sim/` holds working stimulus files with the BASIC programs they go with: `serial_rx.stim.toml` (three good characters then one with a bad Stop bit), `adc_q41.stim.toml` (a channel stepping from 0xABC to 0x123 at cycle 600), `i2c_q10.stim.toml`, `spi_q10.stim.toml`, `spi_q41.stim.toml`, `blink.stim.toml` and `board_example.stim.toml`. `tests/sim/run.sh` runs them (it needs a compiled `basil`).

---

# 8. The simulated board: LEDs and OLED

The simulator can attach two kinds of part to pins: LEDs and an SSD1306 OLED display. Describe them in the stimulus file, or in the **Board** tab of `pic-sim`.

## LEDs

```toml
[[led]]
pin = "RB0"        # the pin the LED is wired to
active = "high"    # "high" (default): lit when the pin is a high output
name = "heartbeat" # optional
```

`active = "low"` is an LED wired to Vdd through the pin: lit when the pin is a **low** output. An LED is lit only while its pin is an **output** (the TRIS bit is clear) at the active level; an input pin leaves it dark. The window shows brightness from the fraction of time lit; `basil-sim` prints the final state, the time lit and each change with its cycle.

In the Board tab each LED has a name, a pin chosen from the part's pins, an *active high* check box and an `x` to remove it; **Add LED** adds another.

## SSD1306 OLED

```toml
[[oled]]
bus = "i2c1"          # "i2c1"/"i2c2": the part's hardware I2C host; "gpio": bit-banged
scl = "RC3"
sda = "RC4"
address = 0x3C        # 7-bit, default 0x3C
height = 64           # 64 (default) or 32
external_vcc = false  # true: the module has its own VCC and lights without the charge pump
```

The display is 128 columns by 64 or 32 rows. **The pins you give must be the pins the program uses.**

**Hardware bus (`i2c1`, `i2c2`).** The OLED acknowledges only when the program has set up the module's pin selection as it would have to on a real part: the input selectors (`SSPxCLKPPS`, `SSPxDATPPS` on the Q10 and PIC16; `I2CxSCLPPS`, `I2CxSDAPPS` on the Q41) must select the OLED's SCL and SDA pins, and the output selector `RxyPPS` of **both** pins must hold the bus output code (Q10 MSSP1: SCL `0x0F`, SDA `0x10`; MSSP2: `0x11`, `0x12`; Q41 I2C1: SCL `0x21`, SDA `0x22`). The PIC18(L)F2x/4xK22 has no PPS: the OLED must be on the module's own pins (MSSP1: SCL RC3, SDA RC4; MSSP2: RB1, RB2 on the 28-pin parts, RD0, RD1 on the 40/44-pin parts). Otherwise the address gets a NACK, the display does not draw, and the Board tab or `basil-sim` says which register is wrong (`NOT WIRED: …`). A PIC16 part must have the MSSP in I2C host mode for this.

**Bit-banged (`gpio`).** The two pins are watched after every instruction. A start or stop is a change of SDA while SCL is high, a bit is SDA at the rising edge of SCL, and the display pulls SDA low for the acknowledge. A line is low when the PIC drives it low or the display holds it; external pull-ups are assumed, so a released line reads high.

**What the display understands.** The simulator implements the SSD1306 command set from its data sheet: display on/off, contrast, the three addressing modes with the data sheet's pointer rules, column and page range, start line, display offset, multiplex ratio, segment remap, COM scan direction, invert, entire display on, the charge pump, and the horizontal and the vertical-and-horizontal **scroll** commands. Scrolling runs in real time from the `fosc_mhz` clock; at reset one scroll step of five frames is about 47 ms.

**The charge pump.** The panel lights only when the charge pump has been enabled (`8Dh 14h` before `AFh`), or when `external_vcc = true`. A program that turns the display on without either gets a note and a dark panel; the Board tab shows `on, but dark: charge pump (8Dh 14h) not enabled` and an *external VCC* check box.

**Reads.** I2C reads from the display return `0xFF` and a note, because the SSD1306 has no read path in serial mode.

**In the window.** The Board tab has a bus choice (hardware I2C1, hardware I2C2 or bit-banged GPIO), SCL and SDA pin lists, the address (hex), 64 or 32 rows, and the external-VCC option. **Add OLED** is offered when none is wired. Changing a pin or address re-attaches the part: the display starts blank like a power-up. The Board tab can be used before a program is loaded and the wiring is kept when another program is loaded.

---

# 9. What is simulated

## Time

* The unit is the instruction cycle (`Fosc / 4`). Instructions take the cycle counts in the data sheet's instruction tables: one cycle for most; two for branches, calls, returns, table reads and writes and `MOVFF` on PIC18; extra cycles for a taken skip; one extra for a PIC16 access to program flash through an indirect register.
* Peripheral events are scheduled at exact cycles. A timer flag, a UART bit time, an ADC conversion and an EEPROM write complete at the cycle the data sheet gives.
* Interrupt entry follows the data sheet (and was matched against MDB): the vector is reached a fixed number of cycles after the flag is set and after the instruction in progress completes.

## Cores

| | PIC18 | PIC16 enhanced mid-range |
|---|---|---|
| Instruction set | Base set plus the Q41 additions (`MOVFFL` …) | The 50-instruction set including `MOVIW`, `MOVWI`, `BRW`, `CALLW`, `ADDFSR` |
| Program counter | Bytes, 21 bits | Words, 15 bits |
| Stack | Hardware, 31 levels (Q10, K22), overflow/underflow reset with `STVREN` | 16 levels; the 17th push overflows; `STKPTR` as the data sheet |
| Interrupts | Q10 and K22: legacy, optional two priorities, vectors 0008h and 0018h (K22: IPEN in RCON, TMR0/INT0/RB/INT1/INT2 in INTCON and INTCON3). Q41: vector table (`MVECEN` = 1) or the two legacy vectors at `IVTBASE` (`MVECEN` = 0), hardware context save | Single vector 0004h, automatic save of W, STATUS, BSR, FSRs and PCLATH to the shadow registers |
| Data memory | Banked with Access Bank, FSR0–2 | Banked (6-bit or 5-bit BSR), linear memory through the FSRs, flash read through an FSR |

## Peripherals

Models exist for the following; a register of anything else is plain storage that honours the data sheet's implemented-bit and read-only masks and is reported as "unmodelled SFRs touched" when the program uses it.

| Peripheral | PIC18-Q10 | PIC18-Q41 | PIC18-K22 | PIC16 |
| --- | --- | --- | --- | --- |
| Ports, `PORTx/LATx/TRISx/ANSELx`, PPS and `PPSLOCK` | yes | yes | ports (no PPS) | yes |
| Interrupts, `SLEEP` and wake-up | yes | yes | yes | yes |
| Timer0, Timer1, Timer2 (Timer3–6 where present) | yes | yes | yes: the older Timer0 (FOSC/4), Timer1/3/5 (FOSC/4, FOSC, SOSC; Timer1 also its T1CKI pin RC0 and gate pin T1G RB5), Timer2/4/6 | yes. Timer0 and Timer1/3/5 can count the edges of their clock pin (`T0CKIPPS`, `TxCKIPPS`) and Timer1/3/5 can be gated by `TxGPPS`; the pin is driven by the stimulus file |
| EUSART / UART | EUSART1/2, asynchronous | UART1–3 | EUSART1/2, asynchronous | EUSART, asynchronous |
| ADC | yes | yes (12-bit) | yes (10-bit, ACQT acquisition) | yes (several variants) |
| Data EEPROM | yes | yes | yes (EECON1) | yes |
| Program flash self-write | yes | yes | yes (64-byte blocks, TBLWT holding registers) | yes |
| MSSP (SPI and I2C host) | yes | — | MSSP1/2 | I2C host mode, to drive the OLED |
| SPI host | via MSSP | yes | via MSSP | — |
| I2C host | via MSSP | yes | via MSSP | — |
| CCP1/2 and PWM3/4 | yes | CCP1 capture, compare and PWM; standalone 16-bit PWM1–3 (see below) | CCP1 to CCP5, PWM mode only (single output) | CCP1 to CCP4 (capture, compare, standard PWM) on PIC16F131xx, 132xx, 152xx, 171xx and 183xx; standalone 10-bit PWM on 131xx, 152xx, 180x6 and 183xx; standalone two-output 16-bit PWM1/2 on 171xx and 132xx (see below) |
| CWG1 | yes | yes, with source limits below | — | — |
| Comparators C1/C2 | yes | yes, Q41 analog pin mapping | — | — |
| CLC1–CLC8 | yes | CLC1–CLC4 through CLCSELECT | — | — |
| CLB (PIC16F131xx, 132xx) | — | — | — | yes: 32 BLEs, input edge detectors, 3-bit counter, software inputs and outputs, PPS outputs and interrupts, driven by a `[clb]` netlist in the stimulus file (section 10); the Synthesizer's bitstream is not read; no clock, gate or trigger connections to other peripherals |
| Watchdog (windowed on Q10 and Q41) and the resets it causes | yes | yes | yes: WDTEN, SWDTEN, configuration postscaler | yes: windowed WWDT (PIC16F131xx, PIC16F132xx, PIC16F171xx) or single-register WDT (PIC16F152xx, PIC16F183xx, PIC16F180x6) |
| CRC and memory scanner | yes | 1–32-bit CRC with flash/EEPROM scanner, priority arbitration and selected triggers | — | — |
| DMA | — | four channels, with limits below | — | — |
| External interrupts INT0–2, interrupt-on-change | yes | yes: PPS pin routing, edge selection, Sleep wake-up and legacy/vectored interrupt dispatch | fixed INT0–2 pins and PORTB mismatch interrupt; Sleep wake-up | the INT pin and interrupt-on-change |
| MCLR pin reset | yes | yes | yes | the MCLR pin is part of the port model |
| LEDs and SSD1306 OLED | yes | yes | yes | yes |
| Firmware oscillator frequency and status | yes | yes | yes: FOSC, OSCCON.IRCF/SCS, OSCTUNE.PLLEN and OSTS/HFIOFS/PLLRDY | yes |

Where this table says "—", `docs/FACTS-SIM.md` documents no model for that peripheral on that family: its registers are plain storage and the program is reported as touching unmodelled SFRs. The Q10 models have their own data sheet citations; shared watchdog behaviour and Q41 pin-interrupt routing are also covered in that document. `docs/FACTS-SIM.md` is the authoritative list, section by section, with the data sheet page for each rule; peripherals are added over time.

**Not modelled anywhere:** the pin waveforms and an external clock of synchronous EUSART mode (its register-level behaviour with byte stimuli is modelled), input thresholds, oscillator trim and fail-safe clock monitoring, and the glitch-free pause of a K22 clock switch (the start-up and PLL lock delays of a switch are modelled; `CSWEN` and `CSWHOLD` are honoured), brown-out, `LVP`, code and write protection.

**Q41 DMA.** All four DMA banks are selected through `DMASELECT` (0–3). The engine reads GPR/SFR, program flash or data EEPROM and writes GPR/SFR through their normal peripheral models. It supports fixed, incrementing and decrementing pointers, independent source/destination counts and reloads, software DGO start/stop, hardware start/abort requests, source/destination stop controls, completion/abort/overrun flags, and PMD. A software stop preserves the buffer and counters; disabling/re-enabling initializes a fresh transfer. Peripheral requests work independently of PIE/GIE. UART/SPI/I2C RX/TX requests are level sensitive; timer and other modelled event requests are pulses, including repeated events while the PIR flag remains set. Each byte uses separate read and write cycles, observable through XIP and BUF.

DMA arbitration honors `MAINPR`, `ISRPR` and `DMA1PR`–`DMA4PR`: lower values win; equal values favor CPU execution, then lower channel numbers. Transfers require `PRLOCKED = 1`, set through writes of 55h, AAh, then 01h to `PRLOCK`. Priorities cannot change while locked. The same key sequence followed by 00h unlocks them when configuration `PR1WAY = 0`; with `PR1WAY = 1`, a reset is required. Higher-priority DMA stalls the CPU for each bus phase. Lower-priority DMA uses available data-bus cycles of literal/control instructions, branch/skip flush cycles for flash reads, and available EEPROM cycles. Idle releases both buses. Multi-cycle file/table instructions are conservatively reserved. CPU instructions execute atomically; DMA effects from shared cycles commit at instruction boundaries, so sub-instruction ordering and NVM/scanner arbitration are not cycle exact. DMA pauses when Fosc stops in Sleep and runs in Idle. DMA-only register mirrors at 4000h and above and DMA-to-DMA control-register access are not modelled; a note identifies such access. The hardware stop pipeline is approximated by stopping at the next simulated bus phase. Unsupported trigger sources do not generate requests. This model validates transfer behavior and the supported priority/bus scheduling; it does not reproduce every system-arbiter timing detail.

**Q41 shared peripherals.** CCP1 uses CCPTMRS0 to choose Timer1/2 or Timer3/4; capture currently accepts the CCP1PPS pin only, and the existing compare pulse and Fosc-clocked PWM limits apply. Comparators use Q41 pins (C1 positive RA0, C2 positive RC0; negative RA1/RC1/RC2/RC3), FVR or VSS; DAC input, hysteresis and response delays remain unmodelled. CLCSELECT selects four independent banks of CLCn registers. Supported CLC inputs are CLCIN0–3, CCP1, modelled PWM outputs, comparators, other CLCs and CWG1A/B; other inputs read low with a note. CWG supports the shared output, steering, bridge, dead-band and software-shutdown modes, with Q41 source selection and PPS codes. Its pin/comparator shutdown sources are supported; timer/CLC shutdown sources and external data input remain unmodelled.

**Q41 16-bit PWM and CRC.** PWM1–3 each have two outputs, an independent 16-bit timer, FOSC clock selection (`PWMxCLK = 2`), clock prescaling, period interrupt postscaling, polarity, left/right/center/variable aligned and pulsed/toggled compare modes, and push-pull in aligned modes. PR/P1/P2 load on enable and remain buffered until LD takes effect at a period boundary; PWMEN/PWMLOAD mirror the enable/load controls. Other clocks, external reset and automatic load sources are not modelled and produce notes. Period/parameter events can trigger DMA. PWM reload latency is idealized; the hardware's three-clock load-request exclusion is not modelled.

The Q41 CRC accepts software data words of 1–32 bits, programmable polynomial/seed, MSb/LSb input order, augmented/unaugmented operation, a held input word, BUSY/FULL, completion interrupt and the CRCOUT/CRCSHIFT/CRCXOR register overlay (SETUP in CRCCON0 bits 3:2). Timing assumes one shifted bit per instruction cycle. The memory scanner feeds flash or EEPROM into CRC, with SCANPR arbitration against CPU/ISR/DMA, priority-lock enforcement and burst/concurrent modes. LFINTOSC, Timer0 and CLC1–4 triggers are supported; unsupported triggers wait with a diagnostic. Transfer timing assumes one byte per granted instruction cycle; wider words are assembled little-endian and a final partial word is zero-filled. Exact Q-clock ordering and NVM-busy contention are not modelled. DMA can feed CRCDATA through the normal SFR path. These new models have regression tests on all six Q41 parts; they have not been measured against silicon.

**PIC16 standalone PWM.** The 10-bit modules use Timer2/4/6 selected by `PWMTMRS(0)` where present, with Fosc/4, prescaling, duty buffering at period rollover, polarity, read-only OUT, PMD and PPS output. Duty is `PWMxDCH:PWMxDCL[7:6]`. Zero duty stays inactive and duty at or above the period stays active. Timer stop and Sleep hold the output and remaining pulse time. Unsupported timer modes/clocks are diagnosed; reserved timer selections produce no waveform. Pulse edges are sampled at instruction boundaries, rounding fractional-cycle widths upward.

PIC16F171xx and PIC16F132xx PWM1/2 share the 16-bit engine described above: two outputs each, FOSC clock 2, prescaling/postscaling, buffered PR/P1/P2 with LD, polarity, left/right/center/variable alignment, compare-pulse/toggle and push-pull. Period and parameter interrupts, PMD, PPS and Sleep are supported. External reset, automatic load and other clocks remain unmodelled and produce notes. PIC16 pins sample the output at instruction boundaries; this does not expose pulses that rise and fall entirely within one instruction cycle.

**The watchdog on PIC18-Q10 and Q41 parts.** The shared windowed watchdog model follows `WDTE`, `WDTCPS`, `WDTCWS` and `WDTCCS` configuration settings. Time-outs while awake reset the part; time-outs in Sleep wake it. Window violations also reset the part, and software enable, `CLRWDT` and reset-cause flags follow the model. All six Q41 variants are covered. Timing uses the effective firmware clock or explicit Fosc override. K22 uses its own WDTEN modes and a nominal 4 ms base with the configuration postscaler. Time-outs reset while awake and wake in Sleep; CLRWDT, SLEEP and IRCF changes clear the counter. K22 TO/PD flags live in RCON. INT0–2 use RB0–2 and INTEDG0–2; IOCB enables PORTB<7:4> mismatch interrupts. Reading or writing PORTB refreshes the mismatch latch; debugger peeks and MOVFF accesses do not. A mismatch reasserts RBIF until acknowledged. Sub-cycle edge/flag-clear collisions are not reconstructed.

**The watchdog on a PIC16 part.** It times out and resets the part (TO and `RWDT` cleared) when the configuration enables it (`WDTE`), and a time-out in Sleep wakes the part with TO = PD = 0. A program that is built without `WDTE` set to off therefore has a watchdog: an erased configuration word enables it, with the default period of 2 s, and a program that never executes `CLRWDT` is reset after 2 s of simulated time. The period is counted in simulated instruction cycles from the clock the stimulus names (`fosc_mhz` override, or the clock derived from firmware when no override is supplied): at the wrong `fosc_mhz` the time-out arrives at the wrong simulated time. On the windowed parts (PIC16F131xx, PIC16F171xx) a `CLRWDT` that the window does not allow, or one that did not follow a read of `WDTCON0` while a window is set, resets the part and clears `WDTWV`. `WDTCON` of the PIC16F152xx is documented with a reset value of 0 but with a default period of 2 s: the simulator uses the 2 s. The PIC16F180x6 watchdog retains the original device-file timing assumptions; DS40002325C is now available for further validation. The watchdog clock is always LFINTOSC (31 kHz, or 31.25 kHz where `WDTCON.CS` says MFINTOSC); EXTOSC and SOSC are not modelled.

**Pins on a PIC16 part.** GPIO and modelled PPS outputs (CCP/PWM, CLB and asynchronous EUSART) feed timer clock/gate, capture, IOC and INT inputs. Select the output's pin in an input PPS register to loop it back. GPIO and CCP/CLB changes reach edge consumers on the instruction clock following the boundary where they are observed; pulses within one instruction are not reconstructed. EUSART bit boundaries follow the configured baud rate. Set the TX pin's `RxyPPS` to its family's TX code and the receiver's `RXnPPS` (`RXPPS` on PIC16F183xx) to the desired digital pin. SPEN/TXEN enable the TX driver automatically. RX supports eight/nine bits, framing errors and overrun recovery; `[[uart]]` and interactive input inject characters directly and take precedence while active.

PIC16 and PIC18 `WPUx` raise otherwise undriven inputs; an explicit low stimulus wins. `ODCONx` makes a high output release the pin and a low output sink. A TRIS output's weak pull-up is disabled even while open drain releases it; use an external pull-up, such as the board OLED's bus pull-ups. ANSEL disables digital PORT/PPS readback while the board still sees the physical output. PIC18 K22 also honors the global active-low RBPU enable in INTCON2.

**CCP on a PIC16 part.** CCP1 to CCP4 drive the pin whose `RxyPPS` holds the CCP's output code while that pin is an output (TRIS bit clear); PORTx and the board's LEDs see it. Compare and PWM run on the same Timer1/Timer2 as the program (CCPTMRS picks the timer). Compare and PWM timing follow MDB where the data sheet's equations differ from it: a compare match is the Timer1 clock after TMR1 reached CCPR; after a match of MODE 0001 or 1011 TMR1 restarts at 1, so the period is CCPR clocks; a PWM pin is set at the first cycle of the last Timer2 count and stays set for `ceil(duty / 4) + 1` counts (one count is 2^n cycles for a 1:2^n prescaler). Not modelled: the compare pulse modes (MODE 1010, 1011 set the flag only), capture sources other than the `CCPxPPS` pin, capture in Sleep, a timer clocked from Fosc, and the PIC16F180x6 (no output code table). CCPxIF in PWM mode is set when the pulse ends.

## Configuration words

`RSTOSC` sets the reset values of `OSCCON1`, `OSCCON2` and `OSCFRQ`; `WDTCPS`, `WDTCWS` and `WDTCCS` set the watchdog registers' reset values; on a PIC16 `MCLRE`, `PPS1WAY` and `STVREN` take effect, and `STVREN` also decides whether a PIC18 stack overflow resets the part. `LVP`, code protection and write protection have no effect. `basil-sim --config` (PIC18 only) prints every field and the registers derived from them.

## Deliberate differences from MDB

The simulator follows the data sheet. Where it differs from MDB it says so in `docs/FACTS-SIM.md`:

* **D-SIM-1:** MDB loses part of the Timer0 prescaler count on a read of `TMR0L`; the data sheet clears the prescaler on writes only.
* **D-SIM-2 (fixed):** the Q10/K22 core now runs the already-fetched instruction before an interrupt entry, as MDB and the PIC16 core do; a few phases of the `bcf INTCON,7` race still differ in the stacked address (`docs/FACTS-SIM.md`).
* **D-SIM-18:** MDB counts the Q10 `ADACQ` in instruction cycles and converts in about 13.5 TAD; the simulator follows the register description (ADC clocks, 11.5 TAD).
* **D-SIM-5:** on PIC16F183xx with `PR2 = 0` MDB never sets the Timer2 flag; the simulator does, every prescaler × postscaler cycles, as the data sheet says.
* MDB has no write time for EEPROM and flash; the simulator does, so cycle counts differ there while results agree.

---

# 10. Troubleshooting

| Symptom | Cause and remedy |
|---|---|
| The device box is empty | No `devices/` folder was found (section 2). Set `PICSIM_DEVICES` or copy `devices/` beside the executable |
| `NAME is a routine in more than one file (a.c, b.c): name one as NAME@a.c` (or `a variable`) | Two C files each define a `static` of that name, and `basil-sim` will not pick one. Add the file, as the message shows: `--until helper@main.c`, `--dump count@util.c` |
| `symbol NAME not found (no prog.S)` | The symbol given with `--until` or `--bp` is in neither the listing nor the ELF. Build with `-g` (for C also `-O0`, so the routine is not inlined away), or check the spelling. A C routine is named as in the source (`add`), without XC8's leading underscore |
| `basil-sim` runs for a long time and exits with status 1 | The program never reaches `__halt` (a main loop). Add `--max-cycles` for the time you want to simulate |
| `unknown option`, `needs a value` | Check the option spelling; `--dump`, `--bp` and `--until` take a value |
| `no .elf beside the hex: no source view or variables` | Build with `basil -g` and keep the `.elf` beside the `.hex` (or one folder up from a `dist/` hex) |
| `ELF has no source lines` | The ELF came from a build without `-g`, or from a part for which the compiler emits no line information. Disassembly view, SFRs and Watch still work |
| `<part>: …` error when loading a stimulus | A section or key the part cannot honour, or a pin the part lacks (section 7). Read the message; it names the section |
| The LED never lights | The pin is an input (TRIS set), is on the wrong port, or `active` is wrong. In the program, set `TRISx` and check `ANSELx` |
| The OLED shows `NOT WIRED` | For a hardware bus the program's PPS set-up does not match the pins you gave: the message names the register. Check `SSPxCLKPPS`/`SSPxDATPPS` (or `I2CxSCLPPS`/`I2CxSDAPPS`) and both `RxyPPS` registers |
| The OLED is on but dark | The charge pump was not enabled (`8Dh 14h`) and `external_vcc` is false |
| Serial input is garbled or lost | Your `bit` or the program's baud rate differs; with `bit` omitted the sender uses the receiver's own rate. The receiver must be enabled (`SPEN`, `CREN`) before the Start bit falls |
| An input pin always reads 0 | Nothing drives it in the stimulus (`[[pin]]`/`[[port]]`), or `ANSELx` has the bit set |
| A flag never sets | The peripheral may be unmodelled; look for `unmodelled SFRs touched` in the notes |
| `the PC ran past the end of program memory` | The program does not end in a loop (or the hex is empty: `the hex file has no program bytes`); the PC went through erased flash and wrapped to 0 |
| The program runs wild from reset | The hex may be built for another part: look for the notes `… hex byte(s) at … are outside every memory region of the …` and `the ELF was built for … but the part is the …`. A PIC16 hex on a large PIC18 part can fit its memory without either note when there is no ELF |
| Results differ from the real chip | See the notes and section 9. If a behaviour matters and is marked `[A#]` in `docs/FACTS-SIM.md`, it is an assumption not yet measured on silicon |

---

# 11. Appendix A: quick reference

## pic-sim controls

| Where | Control | Effect |
|---|---|---|
| Toolbar | Run / Pause | Start, stop a run or step |
| Toolbar | Step / Step over | One instruction / run over a call |
| Toolbar | Line / Line over / Out | Source-line stepping (needs `basil -g`) |
| Toolbar | Reset | Restart the part, replay the stimulus, keep the wiring |
| Toolbar | Load | (Re)load the program and stimulus; clears breakpoints |
| Toolbar | Stim… / x | Choose a stimulus file (opens in its folder, the program's, or `tests/sim`) / clear it |
| Toolbar | Speed, Fosc override MHz | Run speed; zero derives the clock from firmware, a positive value overrides it |
| Program pane | click a line | Toggle a breakpoint |
| Program pane | follow PC, stay in my code, goto/Go | View control |
| CPU panel | Stopwatch Start/Stop/Lap/Clear | Time simulated intervals |
| CPU panel | Edit… | Open the Debugger tab |
| Right tabs | Memory, SFRs, Watch, Board, Variables, UART, Debugger, Pins, Peripherals, Waves | Inspection, register editing, wiring, terminal, debugging, live pins, decoded peripherals and waveforms |
| Debugger | Write / Drive pin / Apply | Edit a data byte or PC; change digital or ADC input while paused |
| Debugger | Break on value change / Remove | Add or remove a data watchpoint |
| Debugger | Break when: target, comparison, value, at, hit, **Add condition** / Remove | Stop when a byte or 16-bit value satisfies a comparison (edge-triggered), or at an address when it holds there; stop on the Nth hit |
| Debugger | Trace data byte / Trace pin; Record / Stop capture / Clear capture | Choose signals and control waveform capture |
| Debugger, Waves | Export VCD… | Save the waveform with simulated nanosecond timestamps |
| Memory, SFRs, Pins, Peripherals | orange | Changed since the last Step or Run began; SFRs also has **changed only** |
| SFRs | right-click a name | Trace the register in Waves |
| Pins | click an input / right-click a pin | Drive the input to the opposite level (when stopped) / trace the pin in Waves |
| Peripherals | **Fire**, **Clear**, also enable it | Set or clear an interrupt flag, optionally with its enable and the global enables |
| Waves | scroll, drag, Zoom in/out, Fit, hover | Zoom about the pointer, pan, rescale, read values at the cursor |
| Window | drop `.hex`/`.elf`/`.toml` | Load the program / set the stimulus |

## basil-sim options

`--device`, `--devices`, `--asm`, `--until`, `--max-cycles`, `--stim`, `--dump`, `--bp`, `--bp-max`, `--trace`, `--trace-regs`, `--config`, `--speed`, `--fosc`, `--strict`, `--help`, `--version`. Environment: `PICSIM_DEVICES`.

## Stimulus sections

`[[pin]]`, `[[port]]`, `[[bounce]]`, `[[clock]]`, `[[uart]]`, `[[uart_device]]`, `[[adc]]`, `[[analog]]`, `[[i2c]]`, `[[spi]]`, `[[mclr]]`, `[[led]]`, `[[oled]]`, `[clb]`, and the top-level `fosc_mhz`.

## Pin names

`R<port><bit>` with port `A` to `H` and bit `0` to `7`: `RA0`, `RC3`, `RB7`. Only the pins the selected part has are valid.

## Run speeds

`Max`, `1x (real time)`, `1/2`, `1/4`, `1/10`, `1/100`, `1/1000` in the window; `max`, or any ratio such as `1`, `0.5`, `1/10` on the command line.

## Exit status of basil-sim

0 reached `--until` or completed a run with no stop symbol; 1 stopped for another reason; 2 error; 3 unmodelled SFR access in strict mode.

---

# 12. Appendix B: example session

The repository's `crates/picsim/tests/data/zed_g.hex` is a clock/calendar program for an external I2C real-time clock chip, built with `basil -g`. Running it on a simulated PIC18F27Q10 with nothing attached to the I2C bus:

```
$ basil-sim crates/picsim/tests/data/zed_g.hex --device 18F27Q10 --max-cycles 5000000
stop: Cycles  cycles: 5000001  instructions: 3334050
tx1: \r\nMCP7940N clock/calendar\r\nRTCC not answering, status 3\r\n
i2c1: S 6FW! P S 6FW! P S 6FW! P S 6FW! P
note: unmodelled SFRs touched: RX1PPS SSP1CLKPPS SSP1DATPPS
```

Reading it:

* The program did not halt in the 5 000 000 cycles allowed (`stop: Cycles`), which is 0.31 s at 64 MHz, and `basil-sim` would exit with status 1.
* It printed its banner on the serial port and then reported that the real-time clock chip did not answer (`tx1`).
* The `i2c1` line shows why: four times the host started a transfer to address 6F for writing and the byte was not acknowledged (`!`), followed by a stop. No device listens at that address.
* Three SFRs are listed as unmodelled for this part; they belong to peripherals whose models do not cover those registers.

To make the chip answer, give the I2C bus a client in a stimulus file:

```toml
# rtcc.stim.toml
[[i2c]]
address = 0x6F
data = [0x00, 0x00, 0x00]
```

and run again with `--stim rtcc.stim.toml`. The `!` marks disappear from the `i2c1` line and the program carries on to set the clock.

To look at the same program in the window:

```
pic-sim crates/picsim/tests/data/zed_g.hex
```

The device is taken from the ELF, the Source pane opens at the program's `.bas` file, and the Variables tab shows its globals. Click the line that sends the banner, press **Run**, and the program stops there; press **Line over** to step to the next line and watch the orange variables change.
