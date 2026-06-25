use std::io::Cursor;
use crate::vm2;

#[test]
fn test_init_signals_with_buses() {
    let inputs_objects = r#"{
"a": [ [1, 2, 3], [4, 5, 6] ],
"b": [7, 8, 9, 10, 11, 12],
"c": {
    "start": [ { "x": [13, 14], "y": 15 }, { "x": [16, 17], "y": 18 } ],
    "end": { "x": [19, 20], "y": 21 }
},
"d": [
    {
        "start": [ { "x": [22, 23], "y": 24 }, { "x": [25, 26], "y": 27 } ],
        "end": { "x": [28, 29], "y": [30] }
    },
    {
        "start": [ { "x": [31, 32], "y": [33] }, { "x": [34, 35], "y": 36 } ],
        "end": { "x": [37, 38], "y": 39 }
    }
]
}"#.to_string();

    let inputs_bus_as_array = r#"{
"a": [ [1, 2, 3], [4, 5, 6] ],
"b": [7, 8, 9, 10, 11, 12],
"c": {
    "start": [ [13, 14, 15], { "x": [16, 17], "y": 18 } ],
    "end": { "x": [19, 20], "y": 21 }
},
"d": [
    {
        "start": [ { "x": [22, 23], "y": 24 }, { "x": [25, 26], "y": 27 } ],
        "end": { "x": [28, 29], "y": [30] }
    },
    {
        "start": [ { "x": [31, 32], "y": [33] }, { "x": [34, 35], "y": 36 } ],
        "end": { "x": [37, 38], "y": 39 }
    }
]
}"#.to_string();

    let inputs_bigger_bus_as_array = r#"{
"a": [ [1, 2, 3], [4, 5, 6] ],
"b": [7, 8, 9, 10, 11, 12],
"c": {
    "start": [ [13, 14, 15], { "x": [16, 17], "y": 18 } ],
    "end": { "x": [19, 20], "y": 21 }
},
"d": [
    [22, 23, 24, 25, 26, 27, 28, 29, 30],
    {
        "start": [ { "x": [31, 32], "y": [33] }, { "x": [34, 35], "y": 36 } ],
        "end": { "x": [37, 38], "y": 39 }
    }
]
}"#.to_string();

    let inputs_single_array = r#"[
    1, 2, 3, 4, 5, 6, 7, 8, 9, "10", 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39
]"#.to_string();

    let inputs = vec![
        inputs_objects,
        inputs_bus_as_array,
        inputs_bigger_bus_as_array,
        inputs_single_array,
    ];

    for inputs in inputs {

        let ff = Field::new(bn254_prime);

        let inputs = Cursor::new(inputs.as_bytes());

        // [
        //   Type { name: "bus_0", fields: [TypeField { name: "x", kind: Ff, offset: 0, size: 1, dims: [2] }, TypeField { name: "y", kind: Ff, offset: 2, size: 1, dims: [] }] },
        //   Type { name: "bus_1", fields: [TypeField { name: "start", kind: Bus(0), offset: 0, size: 3, dims: [2] }, TypeField { name: "end", kind: Bus(0), offset: 6, size: 3, dims: [] }] }
        // ]
        let types = vec![
            vm2::Type{
                name: "bus_0".to_string(),
                fields: vec![
                    vm2::TypeField{
                        name: "x".to_string(),
                        kind: TypeFieldKind::Ff,
                        offset: 0,
                        base_type_size: 1,
                        dims: vec![2],
                    },
                    vm2::TypeField{
                        name: "y".to_string(),
                        kind: TypeFieldKind::Ff,
                        offset: 2,
                        base_type_size: 1,
                        dims: vec![],
                    },
                ],
            },
            vm2::Type{
                name: "bus_1".to_string(),
                fields: vec![
                    vm2::TypeField{
                        name: "start".to_string(),
                        kind: TypeFieldKind::Bus(0),
                        offset: 0,
                        base_type_size: 3,
                        dims: vec![2],
                    },
                    vm2::TypeField{
                        name: "end".to_string(),
                        kind: TypeFieldKind::Bus(0),
                        offset: 6,
                        base_type_size: 3,
                        dims: vec![],
                    },
                ],
            }
        ];

        // a: offset 7, lengths [2,3] → 6 signals (7-12)
        // b: offset 13, lengths [3,2] → 6 signals (13-18)
        // c: offset 19, bus_1 → 9 signals (19-27)
        // d: offset 28, bus_1[2] → 18 signals (28-45)
        let input_infos = vec![
            vm2::InputInfo{
                name: "a".to_string(),
                offset: 6,
                lengths: vec![2, 3],
                type_id: None,
            },
            vm2::InputInfo{
                name: "b".to_string(),
                offset: 12,
                lengths: vec![3, 2],
                type_id: None,
            },
            vm2::InputInfo{
                name: "c".to_string(),
                offset: 18,
                lengths: vec![],
                type_id: Some("bus_1".to_string()),
            },
            vm2::InputInfo{
                name: "d".to_string(),
                offset: 27,
                lengths: vec![2],
                type_id: Some("bus_1".to_string()),
            },
        ];

        let mut component = vm2::Component::new(
            1, 0, vec![], 39,
        45);

        init_signals(inputs, &ff, &types, &input_infos, &mut component).unwrap();

        let mut signals = vec![];
        component.write_all_signals(&mut signals);

        let want = vec![
            None,
            None,
            None,
            None,
            None,
            None,
            Some(U254::from_str_radix("1", 10).unwrap()),
            Some(U254::from_str_radix("2", 10).unwrap()),
            Some(U254::from_str_radix("3", 10).unwrap()),
            Some(U254::from_str_radix("4", 10).unwrap()),
            Some(U254::from_str_radix("5", 10).unwrap()),
            Some(U254::from_str_radix("6", 10).unwrap()),
            Some(U254::from_str_radix("7", 10).unwrap()),
            Some(U254::from_str_radix("8", 10).unwrap()),
            Some(U254::from_str_radix("9", 10).unwrap()),
            Some(U254::from_str_radix("10", 10).unwrap()),
            Some(U254::from_str_radix("11", 10).unwrap()),
            Some(U254::from_str_radix("12", 10).unwrap()),
            Some(U254::from_str_radix("13", 10).unwrap()),
            Some(U254::from_str_radix("14", 10).unwrap()),
            Some(U254::from_str_radix("15", 10).unwrap()),
            Some(U254::from_str_radix("16", 10).unwrap()),
            Some(U254::from_str_radix("17", 10).unwrap()),
            Some(U254::from_str_radix("18", 10).unwrap()),
            Some(U254::from_str_radix("19", 10).unwrap()),
            Some(U254::from_str_radix("20", 10).unwrap()),
            Some(U254::from_str_radix("21", 10).unwrap()),
            Some(U254::from_str_radix("22", 10).unwrap()),
            Some(U254::from_str_radix("23", 10).unwrap()),
            Some(U254::from_str_radix("24", 10).unwrap()),
            Some(U254::from_str_radix("25", 10).unwrap()),
            Some(U254::from_str_radix("26", 10).unwrap()),
            Some(U254::from_str_radix("27", 10).unwrap()),
            Some(U254::from_str_radix("28", 10).unwrap()),
            Some(U254::from_str_radix("29", 10).unwrap()),
            Some(U254::from_str_radix("30", 10).unwrap()),
            Some(U254::from_str_radix("31", 10).unwrap()),
            Some(U254::from_str_radix("32", 10).unwrap()),
            Some(U254::from_str_radix("33", 10).unwrap()),
            Some(U254::from_str_radix("34", 10).unwrap()),
            Some(U254::from_str_radix("35", 10).unwrap()),
            Some(U254::from_str_radix("36", 10).unwrap()),
            Some(U254::from_str_radix("37", 10).unwrap()),
            Some(U254::from_str_radix("38", 10).unwrap()),
            Some(U254::from_str_radix("39", 10).unwrap()),
        ];
        assert_eq!(want, signals);
    }

}

/// Test parsing flat arrays of buses with different sizes (inspired by simple4.circom)
/// bus_0: Simple(2) - 4 signals (a[2], b[2])
/// bus_1: Simple(6) - 12 signals (a[6], b[6])
/// bus_2: Simple(8) - 16 signals (a[8], b[8])
/// in1: 5 bus_0 instances = 20 signals (offset 4-23)
/// in2: 3 bus_1 instances = 36 signals (offset 24-59)
/// in3: 3 bus_2 instances = 48 signals (offset 60-107)
#[test]
fn test_init_signals_flat_bus_arrays() {
    let ff = Field::new(bn254_prime);

    // Flat arrays for each input
    let inputs_json = r#"{
        "in1": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20],
        "in2": [21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56],
        "in3": [57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104]
    }"#;
    let inputs = Cursor::new(inputs_json.as_bytes());

    // bus_0: Simple(2) with a[2] and b[2]
    // bus_1: Simple(6) with a[6] and b[6]
    // bus_2: Simple(8) with a[8] and b[8]
    let types = vec![
        vm2::Type{
            name: "bus_0".to_string(),
            fields: vec![
                vm2::TypeField{ name: "a".to_string(), kind: TypeFieldKind::Ff, offset: 0, base_type_size: 1, dims: vec![2] },
                vm2::TypeField{ name: "b".to_string(), kind: TypeFieldKind::Ff, offset: 2, base_type_size: 1, dims: vec![2] },
            ],
        },
        vm2::Type{
            name: "bus_1".to_string(),
            fields: vec![
                vm2::TypeField{ name: "a".to_string(), kind: TypeFieldKind::Ff, offset: 0, base_type_size: 1, dims: vec![6] },
                vm2::TypeField{ name: "b".to_string(), kind: TypeFieldKind::Ff, offset: 6, base_type_size: 1, dims: vec![6] },
            ],
        },
        vm2::Type{
            name: "bus_2".to_string(),
            fields: vec![
                vm2::TypeField{ name: "a".to_string(), kind: TypeFieldKind::Ff, offset: 0, base_type_size: 1, dims: vec![8] },
                vm2::TypeField{ name: "b".to_string(), kind: TypeFieldKind::Ff, offset: 8, base_type_size: 1, dims: vec![8] },
            ],
        }
    ];

    // Signals 0-2 are outputs (3 total), then 104 input signals starting at offset 4
    // in1: offset 4, 20 signals (4-23)
    // in2: offset 24, 36 signals (24-59)
    // in3: offset 60, 48 signals (60-107)
    let input_infos = vec![
        vm2::InputInfo{ name: "in1".to_string(), offset: 3, lengths: vec![5], type_id: Some("bus_0".to_string()) },
        vm2::InputInfo{ name: "in2".to_string(), offset: 23, lengths: vec![3], type_id: Some("bus_1".to_string()) },
        vm2::InputInfo{ name: "in3".to_string(), offset: 59, lengths: vec![3], type_id: Some("bus_2".to_string()) },
    ];

    // Total signals: 3 (output) + 104 (input) = 107
    // signals_start = 1, number_of_inputs = 104, signals_num = 107
    let mut component = vm2::Component::new(1, 0, vec![], 104, 107);

    init_signals(inputs, &ff, &types, &input_infos, &mut component).unwrap();

    let mut signals = vec![];
    component.write_all_signals(&mut signals);

    // Verify the first 3 signals are None (signals 0-2 are outputs)
    for (i, signal) in signals.iter().enumerate().take(3) {
        assert_eq!(*signal, None, "signal {} should be None", i);
    }

    // Input signals at indices 3-106 should be 1-104
    for (i, signal) in signals.iter().enumerate().take(107).skip(3) {
        let expected_value = (i - 2) as u64; // i=3 -> 1, i=4 -> 2, etc.
        assert_eq!(
            *signal,
            Some(U254::from(expected_value)),
            "signal {} should be {}", i, expected_value
        );
    }
}

// cargo run --package circom-witnesscalc --bin cvm-compile ./tests/data/circuit25_heterogeneous_buses_simple2_cvm/circuit25_heterogeneous_buses_simple2.cvm -o ./tests/data/circuit25_heterogeneous_buses_simple2.wcd
#[test]
fn test_heterogeneous_bus(){
    let wcd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/circuit25_heterogeneous_buses_simple2.wcd");
    let wcd_data = std::fs::read(wcd).unwrap();
    let mut wcd_reader = std::io::Cursor::new(wcd_data.as_slice());
    let prime = read_witnesscalc_vm2_header(&mut wcd_reader).unwrap();
    let want_prime = BigUint::from_bytes_le(&FieldOps::to_le_bytes(&bn254_prime));
    assert_eq!(want_prime, prime);
    let ff = Field::new(bn254_prime);
    let circuit = deserialize_witnesscalc_vm2_body(&mut wcd_reader, ff.clone()).unwrap();
    let component_tree: Component<U254> = build_component_tree(
        circuit.main_template_id, &circuit.templates).unwrap();
    assert_eq!(8,
        component_tree.components[0].as_ref().unwrap().read().unwrap().number_of_inputs);
}
