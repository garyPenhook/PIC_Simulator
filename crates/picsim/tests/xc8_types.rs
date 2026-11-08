//! Structures, unions, enumerations, pointers and bit-fields in variables built with MPLAB XC8 (`xc8-cc -g -O0`).
//! Fixtures `data/xc8_types_q10.*` (PIC18F27Q10) and `data/xc8_types_p16.*` (PIC16F17146) are built from the same program,
//! `data/xc8_types_TAG.c` (`xc8_types_p16.c` is an identical copy of `xc8_types_q10.c`): `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 xc8_types_TAG.c -o xc8_types_TAG.elf`. Both run to the final loop.

use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

/// The debug info and data memory after the program has run into its final loop.
fn run(tag: &str) -> (DebugInfo, Vec<u8>) {
    let base = format!("{DATA}/xc8_types_{tag}");
    let hex = std::fs::read_to_string(format!("{base}.hex")).unwrap();
    if tag == "q10" {
        let mut m = Machine::new(device("18F27Q10")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        ((m.debug.clone()), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    } else {
        let mut m = Machine16::new(device("16F17146")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        ((m.debug.clone()), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    }
}

#[test]
fn structures_unions_enums_and_bit_fields_show_their_members() {
    for tag in ["q10", "p16"] {
        let (d, mem) = run(tag);
        let show = |n: &str| d.vars.iter().find(|v| v.name == n && v.scope.is_none()).unwrap_or_else(|| panic!("{tag}: {n}")).show(&mem);
        assert!(show("pt").starts_with("{x = ") && show("pt").ends_with(", y = -300 (0xFED4)}"), "{tag}: {}", show("pt"));
        assert_eq!(show("sh"), "{origin = {x = 3 (0x03), y = 0 (0x0000)}, tag = {0 (0x00), 107 (0x6B), 0 (0x00)}, c = BLUE (6)}", "{tag}");
        assert_eq!(show("uw"), "{w = 4660 (0x1234), b = {52 (0x34), 18 (0x12)}}", "{tag}: a union's members all start at offset 0");
        assert_eq!(show("col"), "GREEN (5)", "{tag}");
        assert_eq!(show("fl"), "{a = 5, b = 2}", "{tag}: bit-fields");
        assert!(show("pts").starts_with("{{x = 0 (0x00)"), "{tag}: an array of structures");
        let v = d.vars.iter().find(|v| v.name == "sh").unwrap();
        assert_eq!((v.ty.as_str(), v.bytes), ("struct shape", 7), "{tag}");
    }
}

#[test]
fn pointers_show_the_address_and_what_it_points_at() {
    for tag in ["q10", "p16"] {
        let (d, mem) = run(tag);
        let var = |n: &str| d.vars.iter().find(|v| v.name == n).unwrap();
        let data = var("data").addr;
        let p = var("p").show(&mem);
        assert!(p.ends_with("-> 42 (0x2A)") && p.contains(&format!("{:X}", data + 2)), "{tag}: {p}");
        assert_eq!(var("p").ty, "unsigned char *");
        let pp = var("pp").show(&mem);
        assert!(pp.contains("-> {x = ") && pp.ends_with("y = -300 (0xFED4)}"), "{tag}: {pp}");
        // A list node: the pointer to its own type shows the address only (no endless recursion), and NULL is NULL.
        assert!(var("n1").show(&mem).starts_with("{v = 1 (0x01), next = 0x") && !var("n1").show(&mem).contains("->"), "{tag}");
        assert_eq!(var("n2").show(&mem), "{v = 2 (0x02), next = NULL}", "{tag}");
        // With the program's structure types, that pointer shows the node it points at; one level only, so a ring of nodes ends.
        let n1 = var("n1").show_with(&mem, &d.rom_image(&[]), &d);
        assert!(n1.starts_with("{v = 1 (0x01), next = 0x") && n1.ends_with(" -> {v = 2 (0x02), next = NULL}}"), "{tag}: {n1}");
        assert_eq!(var("n2").show_with(&mem, &d.rom_image(&[]), &d), "{v = 2 (0x02), next = NULL}", "{tag}");
        assert_eq!(var("n1").ty, "struct node");
    }
}

#[test]
fn a_ring_of_nodes_shows_one_level() {
    for tag in ["q10", "p16"] {
        let (d, mut mem) = run(tag);
        let var = |n: &str| d.vars.iter().find(|v| v.name == n).unwrap();
        let (n1, n2) = (var("n1").addr as usize, var("n2").addr as usize);
        // n2.next -> n1, so n1 -> n2 -> n1 -> ...
        let w = if tag == "q10" { 3 } else { 2 };
        for i in 0..w {
            mem[n2 + 1 + i] = (n1 as u32 >> (8 * i)) as u8;
        }
        let s = var("n1").show_with(&mem, &d.rom_image(&[]), &d);
        assert_eq!(s.matches("->").count(), 1, "{tag}: {s}");
        assert_eq!(s.matches("next = 0x").count(), 2, "{tag}: the inner node's pointer is the ring, shown as an address: {s}");
    }
}

#[test]
fn a_path_names_a_member_or_element() {
    for tag in ["q10", "p16"] {
        let (d, mem) = run(tag);
        let base = |n: &str| d.vars.iter().find(|v| v.name == n).unwrap().addr;
        assert_eq!(d.path_addr("sh"), Some((base("sh"), 7)));
        assert_eq!(d.path_addr("sh.origin.y"), Some((base("sh") + 1, 2)), "{tag}");
        assert_eq!(d.path_addr("sh.tag[1]"), Some((base("sh") + 4, 1)), "{tag}");
        assert_eq!(d.path_addr("pts[1].x").map(|p| p.1), Some(1));
        assert_eq!(d.path_addr("pts[1].x").map(|p| p.0), Some(base("pts") + 3), "{tag}");
        assert_eq!(mem[d.path_addr("sh.tag[1]").unwrap().0 as usize], b'k');
        assert_eq!(mem[d.path_addr("data[2]").unwrap().0 as usize], 42);
        for bad in ["sh.nope", "sh.tag[3]", "data.x", "nope", "fl.a", "sh.origin[0]"] {
            assert_eq!(d.path_addr(bad), None, "{tag}: {bad}");
        }
    }
}
