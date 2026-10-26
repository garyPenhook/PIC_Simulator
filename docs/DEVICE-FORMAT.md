# Device description format

Device descriptions live in `devices/<part>.toml`. They are **generated** by `tools/devgen` from
Microchip's own device pack and **cross-checked** against numbers read by hand from the data
sheet. They are never edited by hand. The Rust types are in `crates/devdesc` (`Device::load`,
`Device::validate`).

Addresses are plain decimal integers (TOML has no hex for generated tables). Ranges are
`[start, end)` unless a field says `size`.

## Files per device

| File | Made by | Purpose |
|---|---|---|
| `devices/18F27Q10.toml` | `devgen gen` | The description the compiler reads |
| `devices/18F27Q10.expect.toml` | **by hand**, from the data sheet | Values the generated file must agree with; each line carries its page cite |

## Schema (version 0.1)

```toml
[device]
name = "PIC18F27Q10"        # canonical part name
family = "pic18"            # selects the code-generator module
arch = "18xxxx"             # architecture string from the pack
pack = "PIC18F-Q_DFP"       # Microchip pack the file came from
pack_version = "1.31.492"
source = "edc/PIC18F27Q10.PIC"
datasheet = "DS40002043"    # data sheet number given by the pack

[vectors]                   # NOT in the pack: entered per family in devgen, with a data sheet cite
reset = 0
interrupt_high = 8
interrupt_low = 24
source = "…DS40002043F p.108 and p.193…"

[memory]                    # plain values first, then the tables below
hw_stack = 31               # levels of the hardware return stack (pack: hwstackdepth); 0 = not given
[memory.program]  start = 0        size = 131072      # bytes
[memory.eeprom]   start = 3211264  size = 1024
[memory.user_id]  start = 2097152  size = 256
[memory.config]   start = 3145728  size = 12
[memory.device_id] start = 4194302 size = 2  value = 28928

[memory.data]
total_gpr  = 3615           # all general-purpose RAM, incl. system (SECTOR) RAM: matches the data sheet
usable_gpr = 3359           # what the compiler may allocate (total minus system RAM)
access_ram = { start = 0,    end = 96 }       # Access Bank, GPR half
access_sfr = { start = 3936, end = 4096 }     # Access Bank, SFR half (0xF60-0xFFF)
sfr = [ { start = …, end = … }, … ]            # SFR address ranges
[[memory.data.gpr]]  bank = 0  start = 0  end = 256  system = false   # one per bank; bank 13 is system
```

`[[config]]` — one entry per configuration register (`CONFIG1L` … `CONFIG6H`):
`name`, `addr`, `default` (value when unprogrammed), `implemented` (bit mask), and `fields`, each
with `name`, `pos`, `width` and `settings = [{ name, value }]` — the names pic-as accepts in
`CONFIG FIELD = SETTING`.

`[[sfr]]` — one entry per memory-mapped special function register: `name`, `addr`, `width`
(bits), optional `aliases` (legacy names such as `TMR1CON`), `fields` (current data sheet names)
and `legacy` (older names such as `TMR1ON`, when the pack has them). Field `pos` is the bit
position of the least significant bit.

`[[joined]]` — multi-byte registers (`FSR0`, `TMR1`, `NVMADR`, `TBLPTR`): `name`, `addr` (low
byte), `bytes`, and `members` (byte registers, least significant first). The number of implemented
bits can be less than `bytes × 8` (a 22-bit address register spans three bytes).

## What `devgen` does

```
devgen gen   <pack>/edc/<PART>.PIC  devices/<part>.toml     # read pack, validate, write
devgen check devices/<part>.toml    devices/<part>.expect.toml   # compare with the data sheet
scripts/check-devices.sh                                       # both, for every device
```

* `gen` reads the pack's `.PIC` file, builds the description, runs `Device::validate()` (see
  below) and refuses to write an inconsistent file.
* Simulator-internal registers (`TMR0_Internal` …) that have no address are skipped.
* Joined registers are defined by their byte members; the pack's declared widths are not trusted
  (a 22-bit address register is declared 16-bit; some entries list no members).
* Sectors of one region (flash, EEPROM, …) must be contiguous; otherwise `gen` stops rather than
  guess.
* `check` compares sizes, vectors and peripheral counts (found by counting SFR names such as
  `TX#STA`) with the hand-entered expect file, so a pack/data-sheet disagreement is caught.
* The expect file may also hold `hw_stack_levels` (compared with `memory.hw_stack`), `system_ram_bytes`
  (RAM the pack lists beyond the data sheet's `sram_bytes`, such as the PIC18-Q41 NVM page-buffer bank, so
  that `total_gpr = sram_bytes + system_ram_bytes` and `usable_gpr = sram_bytes`), and `[accepted."<what>"]`
  tables (`pack = <value>`, `reason = "…"`) for a difference between the pack and the data sheet that has
  been looked at and does not matter; `check` prints it as a note and fails on any other difference.
* A joined register the pack declares with one byte member (`DAC1DAT` on the PIC18-Q41) is an ordinary
  SFR; `gen` skips it rather than stopping (DS40002214G p.765: `DAC1DATL` is 8 bits).

`Device::validate()` checks: GPR ranges sorted, non-overlapping, and `total_gpr` / `usable_gpr`
consistent; vectors increasing; every SFR inside an SFR range with unique names; fields within
the register width and not overlapping; joined members consecutive and present; config registers
inside the config region and every setting value fitting its field.

## Family differences

* `pic18` parts with the vectored interrupt controller (PIC18-Q41, recognised by the `SHADCON` register): the
  vectors in the file are those of `MVECEN = 0`, the mode the compiler uses; see `docs/FACTS-18F-Q41.md`
  for what else differs (14-bit data addresses, an Access Bank made of two ranges, `INTCON0`, 127-level stack,
  hardware context save). `family_facts()` in `devgen` is keyed by the pack's `arch` (`18xxxx` for both the Q10
  and the Q41), so its vector source cites both data sheets.
* `pic16e` (enhanced mid-range, arch `16Exxx`): one interrupt vector, so `interrupt_high` and
  `interrupt_low` are equal (0004h); there is no Access Bank, so `access_ram` / `access_sfr` are
  empty ranges (`start = end = 0`); program, user-ID and configuration sizes and addresses count
  14-bit **words**, not bytes; GPR sectors that only shadow common RAM are skipped so `total_gpr`
  matches the data sheet.
* Descriptions exist for `PIC16F17126` and `PIC16F17146` (DS40002343F), but there is no `pic16e` code
  generator yet: the compiler refuses these parts with an `E-DEVICE` error (PLAN §9.3 step 3).

## Adding a device (checklist)

1. Get the pack, data sheet and errata (via the `microchip-docs` / `pic-docs` tools) and note the
   pack path.
2. If the pack's `arch` has no entry in `family_facts()` in `tools/devgen/src/main.rs`, add one —
   **only after** reading the vectors and basic memory model in that family's data sheet, and cite
   the page. A new core family also needs a code-generator module (PLAN §9.3).
3. `devgen gen …` → review the diff against a sibling part.
4. Write `devices/<part>.expect.toml` **by hand from the data sheet** (Table 1 values, memory map
   figure, interrupt chapter), each value with its page.
5. `scripts/check-devices.sh` must pass.
6. Read the part's errata; record applicable items in a `docs/FACTS-<part>.md` like
   `docs/FACTS-18F27Q10.md`.
7. Compile the standard test programs for the part; only then list it as supported in
   `docs/DEVICES.md`.
8. **Review every library module for the new part** (`docs/LIBRARY.md`, "Device support"). The modules
   in `lib/` were written for the PIC18F27/47Q10; each one names registers, bit fields, interrupt flags,
   pins and channel numbers that may differ or not exist on the new part. Add the part's column to that
   table, port or `#error` each module, and do not call a module supported until it is. A new *family*
   (not just a new part in a known one) also needs the intrinsics and any inline assembly in the modules
   (`Asm` blocks, table reads) checked against the new instruction set.
9. **Update `docs/MANUAL.md`, "Supported device"**: add the part, its core, memory sizes and its status
   (supported, or described but not compilable and why), and keep the library table in step.
