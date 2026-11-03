use picsim::lines::LineMap;

#[test]
fn elf_line_table_maps_pc_to_basic_lines() {
    let m = LineMap::from_elf(include_bytes!("data/i2c_q10_g.elf")).unwrap();
    assert!(!m.is_empty());
    let bas: Vec<_> = m.source_files().iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert!(bas.contains(&"main.bas".to_string()), "{bas:?}");
    // Every instruction of the program body resolves to a line; the first main.bas row is where the program starts.
    let (f, l) = (0..0x400u32).step_by(2).find_map(|a| m.lookup(a).filter(|(f, _)| f.ends_with("main.bas"))).expect("a main.bas line");
    assert!(f.ends_with("main.bas") && l >= 1);
}

#[test]
fn lookup_is_exact_per_instruction() {
    let m = LineMap::from_elf(include_bytes!("data/i2c_q10_g.elf")).unwrap();
    // readelf --debug-dump=decodedline: main.bas 18 @0x5a, 19 @0x5c, 20 @0x60 (the 0x5c row is a 4-byte instruction)
    for (a, line) in [(0x5a, 18), (0x5c, 19), (0x5e, 19), (0x60, 20)] {
        let (f, l) = m.lookup(a).unwrap();
        assert!(f.ends_with("main.bas"));
        assert_eq!(l, line, "at {a:#x}");
    }
}
