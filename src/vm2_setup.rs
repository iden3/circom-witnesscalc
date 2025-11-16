use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, RwLock};
use crate::field::{FieldOperations, FieldOps};
use crate::vm2::{Component, InputInfo, Template, Type, TypeFieldKind};

/// Initialize signals array with input values from JSON
pub fn init_signals<T: FieldOps, F>(
    inputs_json: impl std::io::Read,
    ff: &F,
    types: &[Type],
    input_infos: &[InputInfo],
    component: &mut Component<T>
) -> Result<(), Box<dyn std::error::Error>>
where
        for <'a> &'a F: FieldOperations<Type = T> {

    // Expand each InputInfo into all its constituent signal paths
    let mut signal_path_to_idx: HashMap<String, usize> = HashMap::new();
    for input_info in input_infos {
        let expanded_paths = expand_input_info_to_signal_paths(input_info, types)?;
        for (path, idx) in expanded_paths {
            signal_path_to_idx.insert(path, idx);
        }
    }

    let input_signals = parse_signals_json(inputs_json, ff)?;

    for (path, value) in input_signals.iter() {
        // Try to find exact match first
        if let Some(&signal_idx) = signal_path_to_idx.get(path) {
            signal_path_to_idx.remove(path);
            component.set_signal(signal_idx-1, *value).map_err(|e| -> Box<dyn Error> {e})?;
            continue;
        }

        // Try converting flat array path to multi-dimensional path
        if let Some(multidim_path) = try_convert_flat_to_multidim_path(path, input_infos) {
            if let Some(&signal_idx) = signal_path_to_idx.get(&multidim_path) {
                signal_path_to_idx.remove(&multidim_path);
                component.set_signal(signal_idx-1, *value).map_err(|e| -> Box<dyn Error> {e})?;
                continue;
            }
        }

        if let Some(field_path) = try_convert_bus_flat_to_field_path(path, input_infos, types) {
            if let Some(&signal_idx) = signal_path_to_idx.get(&field_path) {
                signal_path_to_idx.remove(&field_path);
                // ToDo: Investigate why component signals start expected only in flat buses structures
                component.set_signal(signal_idx + component.signals_start, *value).map_err(|e| -> Box<dyn Error> {e})?;
                continue;
            }
        }

        // Try parsing as array access with [0] suffix for backwards compatibility
        if path.ends_with("[0]") {
            let base_path = path.trim_end_matches("[0]");
            if let Some(&signal_idx) = signal_path_to_idx.get(base_path) {
                signal_path_to_idx.remove(base_path);
                component.set_signal(signal_idx-1, *value).map_err(|e| -> Box<dyn Error> {e})?;
                continue;
            }
        }

        return Err(format!("signal {} is not found in input infos", path).into());
    }

    // Check if any input signals were not provided
    if !signal_path_to_idx.is_empty() {
        let missing_signals: Vec<String> = signal_path_to_idx.keys().cloned().collect();
        return Err(format!("Missing input signals: {}", missing_signals.join(", ")).into());
    }

    Ok(())
}

/// Build the component tree for VM2 execution
pub fn build_component_tree<T: FieldOps>(
    main_template_id: usize, vm_templates: &[Template]) -> Component<T> {

    create_component(main_template_id, 1, vm_templates).0
}

/// Create a component tree and returns the component and the number of signals
/// of self and all its children
fn create_component<T: FieldOps>(
    template_id: usize,
    signals_start: usize, vm_templates: &[Template]) -> (Component<T>, usize) {

    let t = &vm_templates[template_id];
    let mut next_signal_start = signals_start + t.signals_num;
    let mut components = Vec::with_capacity(t.components.len());
    for cmp_tmpl_id in t.components.iter() {
        components.push(match cmp_tmpl_id {
            None => None,
            Some( tmpl_id ) => {
                let (c, signals_num) = create_component(
                    *tmpl_id, next_signal_start, vm_templates);
                next_signal_start += signals_num;
                Some(Arc::new(RwLock::new(c)))
            }
        });
    }
    (
        Component::new(
            signals_start,
            template_id,
            components,
            t.number_of_inputs,
            t.signals_num),
        next_signal_start - signals_start
    )
}

/// Expand an InputInfo into all its constituent signal paths with their indices
fn expand_input_info_to_signal_paths(
    input_info: &InputInfo,
    types: &[Type]
) -> Result<Vec<(String, usize)>, Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    let mut current_offset = input_info.offset;

    if let Some(type_id) = &input_info.type_id {
        // This is a bus type - expand it recursively
        let bus_type = types.iter().find(|t| t.name == *type_id)
            .ok_or_else(|| <&str as Into<Box<dyn std::error::Error>>>::into("bus type not found"))?;

        if input_info.lengths.is_empty() {
            // Single bus instance
            expand_bus_type(&input_info.name, bus_type, types, &mut current_offset, &mut paths)?;
        } else {
            // Array of bus instances
            let total_elements: usize = input_info.lengths.iter().product();
            for i in 0..total_elements {
                let array_path = format!("{}[{}]", input_info.name, i);
                expand_bus_type(&array_path, bus_type, types, &mut current_offset, &mut paths)?;
            }
        }
    } else {
        // This is a field (ff) type
        if input_info.lengths.is_empty() {
            // Single field
            paths.push((input_info.name.clone(), current_offset));
        } else {
            // Array of fields - generate multi-dimensional paths
            let total_elements: usize = input_info.lengths.iter().product();
            for i in 0..total_elements {
                let multi_indices = flat_to_multidim_indices(i, &input_info.lengths);
                let mut array_path = input_info.name.clone();
                for idx in multi_indices {
                    array_path.push_str(&format!("[{}]", idx));
                }
                paths.push((array_path, current_offset));
                current_offset += 1;
            }
        }
    }

    Ok(paths)
}

/// Convert flat array index to multi-dimensional indices
fn flat_to_multidim_indices(flat_idx: usize, dimensions: &[usize]) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut remaining = flat_idx;

    for &dim in dimensions.iter().rev() {
        indices.push(remaining % dim);
        remaining /= dim;
    }

    indices.reverse();
    indices
}

/// Recursively expand a bus type into individual field paths
fn expand_bus_type(
    base_path: &str,
    bus_type: &Type,
    types: &[Type],
    current_offset: &mut usize,
    paths: &mut Vec<(String, usize)>
) -> Result<(), Box<dyn std::error::Error>> {
    for field in &bus_type.fields {
        let field_path = format!("{}.{}", base_path, field.name);

        match &field.kind {
            TypeFieldKind::Bus(bus_type_index) => {
                let field_type = types.get(*bus_type_index)
                    .ok_or_else(|| <String as Into<Box<dyn std::error::Error>>>::into(
                        format!(
                            "bus type not found: bus type index {}",
                            bus_type_index)))?;

                if field.dims.is_empty() {
                    // Single bus instance
                    expand_bus_type(&field_path, field_type, types, current_offset, paths)?;
                } else {
                    // Array of bus instances
                    let total_elements: usize = field.dims.iter().product();
                    for i in 0..total_elements {
                        let array_path = format!("{}[{}]", field_path, i);
                        expand_bus_type(&array_path, field_type, types, current_offset, paths)?;
                    }
                }
            },
            TypeFieldKind::Ff => {
                // This field is a primitive type (ff)
                if field.dims.is_empty() {
                    // Single field
                    paths.push((field_path, *current_offset));
                    *current_offset += 1;
                } else {
                    // Array of fields
                    let total_elements: usize = field.dims.iter().product();
                    for i in 0..total_elements {
                        let array_path = format!("{}[{}]", field_path, i);
                        paths.push((array_path, *current_offset));
                        *current_offset += 1;
                    }
                }
            }
        }
    }

    Ok(())
}

fn try_convert_bus_flat_to_field_path(
    path: &str,
    input_infos: &[InputInfo],
    types: &[Type],
) -> Option<String> {
    let bracket_start = path.find('[')?;
    let base_name = &path[..bracket_start];
    let flat_idx_str = path[bracket_start+1..].strip_suffix(']')?;
    let flat_idx = flat_idx_str.parse::<usize>().ok()?;

    let input_info = input_infos.iter().find(|ii| ii.name == base_name)?;
    let type_id = input_info.type_id.as_ref()?;

    let bus_type = types.iter().find(|t| t.name == *type_id)?;

    let mut paths: Vec<(String, usize)> = Vec::new();
    let mut current_offset = input_info.offset;

    let bus_count: usize = if input_info.lengths.is_empty() {
        1
    } else {
        input_info.lengths.iter().product()
    };

    for i in 0..bus_count {
        let instance_path = if bus_count == 1 {
            base_name.to_string()
        } else {
            format!("{}[{}]", base_name, i)
        };

        let mut tmp_paths: Vec<(String, usize)> = Vec::new();
        let mut local_offset = current_offset;

        expand_bus_type(&instance_path, bus_type, types, &mut local_offset, &mut tmp_paths).ok()?;

        current_offset = local_offset;
        paths.extend(tmp_paths);
    }

    paths.get(flat_idx).map(|(p, _)| p.clone())
}

/// Check if a path represents a flat array access and convert to multi-dimensional path
fn try_convert_flat_to_multidim_path(
    path: &str,
    input_infos: &[InputInfo]
) -> Option<String> {
    // Extract base name and flat index from path like "b[4]"
    if let Some(bracket_start) = path.find('[') {
        let base_name = &path[..bracket_start];
        let bracket_part = &path[bracket_start..];

        // Parse flat index from "[4]"
        if let Some(flat_idx_str) = bracket_part.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Ok(flat_idx) = flat_idx_str.parse::<usize>() {
                // Find matching input info
                for input_info in input_infos {
                    if input_info.name == base_name && input_info.lengths.len() > 1 {
                        // Convert flat index to multi-dimensional indices
                        let multi_indices = flat_to_multidim_indices(flat_idx, &input_info.lengths);

                        // Build multi-dimensional path
                        let mut result = base_name.to_string();
                        for idx in multi_indices {
                            result.push_str(&format!("[{}]", idx));
                        }
                        return Some(result);
                    }
                }
            }
        }
    }

    None
}

fn parse_signals_json<T: FieldOps, F>(
    inputs_data: impl std::io::Read,
    ff: &F) -> Result<HashMap<String, T>, Box<dyn std::error::Error>>
where
        for <'a> &'a F: FieldOperations<Type = T> {

    let v: serde_json::Value = serde_json::from_reader(inputs_data)?;
    let mut records: HashMap<String, T> = HashMap::new();
    visit_inputs_json("", &v, &mut records, ff)?;
    Ok(records)
}

fn visit_inputs_json<T: FieldOps, F>(
    prefix: &str, v: &serde_json::Value, records: &mut HashMap<String, T>,
    ff: &F) -> Result<(), Box<dyn std::error::Error>>
where
        for <'a> &'a F: FieldOperations<Type = T> {

    match v {
        serde_json::Value::Null => return Err(
            format!("unexpected null value at path {}", prefix).into()),
        serde_json::Value::Bool(b) => {
            let b = if *b { T::one() } else { T::zero() };
            if prefix.is_empty() {
                return Err("boolean value cannot be at the root".into());
            }
            records.insert(prefix.to_string(), b);
        },
        serde_json::Value::Number(n) => {
            let v = if n.is_u64() {
                let n = n.as_u64().unwrap();
                ff.parse_le_bytes(n.to_le_bytes().as_slice())
                    .map_err(|e| -> Box<dyn Error> {e})?
            } else if n.is_i64() {
                let n = n.as_i64().unwrap();
                ff.parse_str(&n.to_string())?
            } else {
                return Err(format!("invalid number at path {}: {}", prefix, n)
                    .into());
            };
            if prefix.is_empty() {
                return Err("number value cannot be at the root".into());
            }
            records.insert(prefix.to_string(), v);
        },
        serde_json::Value::String(s) => {
            if prefix.is_empty() {
                return Err("string value cannot be at the root".into());
            }
            records.insert(prefix.to_string(), ff.parse_str(s)?);
        },
        serde_json::Value::Array(vs) => {
            if prefix.is_empty() {
                return Err("array value cannot be at the root".into());
            }
            for (i, v) in vs.iter().enumerate() {
                let new_prefix = format!("{}[{}]", prefix, i);
                visit_inputs_json(&new_prefix, v, records, ff)?;
            }
        },
        serde_json::Value::Object(o) => {
            for (k, v) in o.iter() {
                let new_prefix = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{}.{}", prefix, k)
                };
                visit_inputs_json(&new_prefix, v, records, ff)?;
            }
        },
    };

    Ok(())
}

/// Debug helper to print the entire component tree
#[cfg(feature = "debug_vm2")]
pub fn debug_component_tree<T: FieldOps>(component: &Component<T>, templates: &[Template]) {
    println!("\n=== Component Tree ===");
    print_component_tree(component, templates, 0);
    println!("===================\n");
}

#[cfg(feature = "debug_vm2")]
fn print_component_tree<T: FieldOps>(component: &Component<T>, templates: &[Template], indent: usize) {
    let indent_str = "  ".repeat(indent);
    let template_name = &templates[component.template_id].name;

    println!("Component: {} (signals_start: {}, inputs: {})",
             template_name, component.signals_start, component.number_of_inputs);

    if !component.components.is_empty() {
        println!("{}  subcomponents:", indent_str);
        for (i, sub_component) in component.components.iter().enumerate() {
            match sub_component {
                Some(sub) => {
                    print!("{}  [{}]: ", indent_str, i);
                    print_component_tree(&sub.read().unwrap(), templates, indent + 2);
                }
                None => {
                    println!("{}  [{}]: -", indent_str, i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use num_bigint::BigUint;
    use crate::storage::{deserialize_witnesscalc_vm2_body, read_witnesscalc_vm2_header};
    use crate::vm2::Signal;
    use crate::field::{Field, U254, bn254_prime};

    #[test]
    fn test_build_component_tree() {
        // Create leaf templates with no components
        let template1 = Template {
            name: "Leaf1".to_string(),
            code: vec![],
            signals_num: 3,
            number_of_inputs: 1,
            components: vec![],
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        let template2 = Template {
            name: "Leaf2".to_string(),
            code: vec![],
            signals_num: 3,
            number_of_inputs: 1,
            components: vec![],
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        let template3 = Template {
            name: "Leaf3".to_string(),
            code: vec![],
            signals_num: 3,
            number_of_inputs: 1,
            components: vec![],
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        let template4 = Template {
            name: "Leaf4".to_string(),
            code: vec![],
            signals_num: 3,
            number_of_inputs: 1,
            components: vec![],
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        // Create middle-level templates, each with two children
        // First middle template has two children
        let template5 = Template {
            name: "Middle1".to_string(),
            code: vec![],
            signals_num: 4,
            number_of_inputs: 1,
            components: vec![Some(0), Some(1)], // References to template1 and template2
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        // Second middle template has one child and one None
        let template6 = Template {
            name: "Middle2".to_string(),
            code: vec![],
            signals_num: 4,
            number_of_inputs: 1,
            components: vec![Some(2), None, Some(3)], // References to template3, None, and template4
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        // Create root template with two children
        let template7 = Template {
            name: "Root".to_string(),
            code: vec![],
            signals_num: 5,
            number_of_inputs: 1,
            components: vec![Some(4), Some(5)], // References to template5 and template6
            inputs: vec![Signal::Ff(vec![1])],
            outputs: vec![Signal::Ff(vec![1])],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };

        let vm_templates = vec![
            template1, template2, template3, template4, template5, template6,
            template7];

        // Build component tree with template7 (Root) as the main template
        let component_tree: Component<U254> = build_component_tree(6, &vm_templates);

        // Verify the structure of the root component
        assert_eq!(component_tree.signals_start, 1);
        assert_eq!(component_tree.template_id, 6);
        assert_eq!(component_tree.number_of_inputs, 1);
        assert_eq!(component_tree.components.len(), 2);

        // Verify the first child component (Middle1)
        let middle1 = component_tree.components[0].as_ref().unwrap().read().unwrap();
        assert_eq!(middle1.signals_start, 6); // 1 (start) + 5 (signals_num of root)
        assert_eq!(middle1.template_id, 4);
        assert_eq!(middle1.number_of_inputs, 1);
        assert_eq!(middle1.components.len(), 2);

        // Verify the second child component (Middle2)
        let middle2 = component_tree.components[1].as_ref().unwrap().read().unwrap();
        assert_eq!(middle2.signals_start, 16); // 6 (start of middle1) + 4 (signals_num of middle1) + 3 (signals_num of leaf1) + 3 (signals_num of leaf2)
        assert_eq!(middle2.template_id, 5);
        assert_eq!(middle2.number_of_inputs, 1);
        assert_eq!(middle2.components.len(), 3);

        // Verify Middle2 has a None component
        assert!(middle2.components[1].is_none());

        // Verify the leaf components of Middle1
        let leaf1 = middle1.components[0].as_ref().unwrap().read().unwrap();
        assert_eq!(leaf1.signals_start, 10); // 6 (start of middle1) + 4 (signals_num of middle1)
        assert_eq!(leaf1.template_id, 0);
        assert_eq!(leaf1.number_of_inputs, 1);
        assert_eq!(leaf1.components.len(), 0);

        let leaf2 = middle1.components[1].as_ref().unwrap().read().unwrap();
        assert_eq!(leaf2.signals_start, 13); // 10 (start of leaf1) + 3 (signals_num of leaf1)
        assert_eq!(leaf2.template_id, 1);
        assert_eq!(leaf2.number_of_inputs, 1);
        assert_eq!(leaf2.components.len(), 0);

        // Verify the leaf components of Middle2
        let leaf3 = middle2.components[0].as_ref().unwrap().read().unwrap();
        assert_eq!(leaf3.signals_start, 20); // 16 (start of middle2) + 4 (signals_num of middle2)
        assert_eq!(leaf3.template_id, 2);
        assert_eq!(leaf3.number_of_inputs, 1);
        assert_eq!(leaf3.components.len(), 0);

        let leaf4 = middle2.components[2].as_ref().unwrap().read().unwrap();
        assert_eq!(leaf4.signals_start, 23); // 20 (start of leaf3) + 3 (signals_num of leaf3)
        assert_eq!(leaf4.template_id, 3);
        assert_eq!(leaf4.number_of_inputs, 1);
        assert_eq!(leaf4.components.len(), 0);
    }

    #[test]
    fn test_init_signals() {
        // to regenerate the test data from cvm file into wcd, run:
        // cargo run --package circom-witnesscalc --bin cvm-compile ./tests/vm2_setup/data/test_init_signals__cvm.txt -o ./tests/vm2_setup/data/test_init_signals__bc2.wcd
        let wcd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vm2_setup/data/test_init_signals__bc2.wcd");
        let wcd_data = std::fs::read(wcd).unwrap();
        let mut wcd_reader = std::io::Cursor::new(wcd_data.as_slice());
        let prime = read_witnesscalc_vm2_header(&mut wcd_reader).unwrap();
        let want_prime = BigUint::from_bytes_le(&FieldOps::to_le_bytes(&bn254_prime));
        assert_eq!(want_prime, prime);
        let ff = Field::new(bn254_prime);
        let circuit = deserialize_witnesscalc_vm2_body(&mut wcd_reader, ff.clone()).unwrap();
        let inputs_content = include_str!("../tests/vm2_setup/data/test_init_signals__inputs.json");

        let inputs_reader = std::io::Cursor::new(&inputs_content);

        let mut component = Component::new(
            0, 0, vec![], 0,
            43);

        init_signals(
            inputs_reader, &ff, &circuit.types, &circuit.input_infos,
            &mut component).unwrap();

        let want: Vec<Option<U254>> = vec![
            Some(U254::from_str("1").unwrap()), // 0
            None, // 1
            None, // 2
            None, // 3
            None, // 4
            None, // 5
            None, // 6
            None, // 7
            None, // 8
            None, // 9
            None, // 10
            Some(U254::from_str("1").unwrap()), // 11
            Some(U254::from_str("2").unwrap()), // 12
            Some(U254::from_str("3").unwrap()), // 13
            Some(U254::from_str("4").unwrap()), // 14
            Some(U254::from_str("5").unwrap()), // 15
            Some(U254::from_str("6").unwrap()), // 16
            Some(U254::from_str("7").unwrap()), // 17
            Some(U254::from_str("8").unwrap()), // 18
            Some(U254::from_str("9").unwrap()), // 19
            Some(U254::from_str("10").unwrap()), // 20
            Some(U254::from_str("11").unwrap()), // 21
            Some(U254::from_str("12").unwrap()), // 22
            Some(U254::from_str("13").unwrap()), // 23
            Some(U254::from_str("14").unwrap()), // 24
            None, // 25
            None, // 26
            None, // 27
            None, // 28
            None, // 29
            None, // 30
            None, // 31
            None, // 32
            None, // 33
            None, // 34
            None, // 35
            None, // 36
            None, // 37
            None, // 38
            None, // 39
            None, // 40
            None, // 41
            None, // 42
            None, // 43
        ];

        let mut signals: Vec<Option<U254>> = Vec::new();
        use num_traits::One;
        signals.push(Some(U254::one()));
        component.write_all_signals(&mut signals);

        assert_eq!(signals, want);
    }

    #[test]
    fn test_array_inputs() {
        let inputs_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vm2_setup/data/test_array_inputs__inputs.json");
        let inputs_content = std::fs::read(inputs_path).unwrap();
        let inputs_reader = std::io::Cursor::new(&inputs_content);

        // to regenerate the test data from cvm file into wcd, run:
        // cargo run --package circom-witnesscalc --bin cvm-compile ./tests/vm2_setup/data/test_array_inputs__cvm.txt -o ./tests/vm2_setup/data/test_array_inputs__bc2.wcd
        let wcd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vm2_setup/data/test_array_inputs__bc2.wcd");
        let wcd_data = std::fs::read(wcd).unwrap();
        let mut wcd_reader = std::io::Cursor::new(wcd_data.as_slice());
        let prime = read_witnesscalc_vm2_header(&mut wcd_reader).unwrap();
        let want_prime = BigUint::from_bytes_le(&FieldOps::to_le_bytes(&bn254_prime));
        assert_eq!(want_prime, prime);
        let ff = Field::new(bn254_prime);
        let circuit = deserialize_witnesscalc_vm2_body(&mut wcd_reader, ff.clone()).unwrap();
        let mut component = Component::new(
            0, 0, vec![], 0,
            18);

        init_signals(
            inputs_reader, &ff, &circuit.types,
            &circuit.input_infos, &mut component).unwrap();

        let mut signals = Vec::new();
        use num_traits::One;
        signals.push(Some(U254::one()));
        component.write_all_signals(&mut signals);

        // Expected result
        let want: Vec<Option<U254>> = vec![
            Some(U254::from_str("1").unwrap()), // 0
            None, // 1
            None, // 2
            None, // 3
            None, // 4
            None, // 5
            None, // 6
            Some(U254::from_str("1").unwrap()), // 7
            Some(U254::from_str("2").unwrap()), // 8
            Some(U254::from_str("3").unwrap()), // 9
            Some(U254::from_str("4").unwrap()), // 10
            Some(U254::from_str("5").unwrap()), // 11
            Some(U254::from_str("6").unwrap()), // 12
            Some(U254::from_str("7").unwrap()), // 13
            Some(U254::from_str("8").unwrap()), // 14
            Some(U254::from_str("9").unwrap()), // 15
            Some(U254::from_str("10").unwrap()), // 16
            Some(U254::from_str("11").unwrap()), // 17
            Some(U254::from_str("12").unwrap()), // 18
        ];

        assert_eq!(signals, want);
    }

    #[test]
    fn test_parse_signals_json() {
        let ff = Field::new(bn254_prime);

        // bools
        let i = r#"
{
  "a": true,
  "b": false,
  "c": 100500
}"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a".to_string(), U254::from_str("1").unwrap());
        want.insert("b".to_string(), U254::from_str("0").unwrap());
        want.insert("c".to_string(), U254::from_str("100500").unwrap());
        assert_eq!(want, result);

        // embedded objects
        let i = r#"{ "a": { "b": true } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a.b".to_string(), U254::from_str("1").unwrap());
        assert_eq!(want, result);

        // null error
        let i = r#"{ "a": { "b": null } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff);
        let binding = result.unwrap_err();
        assert_eq!("unexpected null value at path a.b", binding.to_string());

        // Negative number
        let i = r#"{ "a": { "b": -4 } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a.b".to_string(), U254::from_str("21888242871839275222246405745257275088548364400416034343698204186575808495613").unwrap());
        assert_eq!(want, result);

        // Float number error
        let i = r#"{ "a": { "b": 8.3 } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff);
        let binding = result.unwrap_err();
        assert_eq!("invalid number at path a.b: 8.3", binding.to_string());

        // string
        let i = r#"{ "a": { "b": "8" } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a.b".to_string(), U254::from_str("8").unwrap());
        assert_eq!(want, result);

        // array
        let i = r#"{ "a": { "b": ["8", 2, 3] } }"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a.b[0]".to_string(), U254::from_str("8").unwrap());
        want.insert("a.b[1]".to_string(), U254::from_str("2").unwrap());
        want.insert("a.b[2]".to_string(), U254::from_str("3").unwrap());
        assert_eq!(want, result);

        // buses and arrays
        let i = r#"{
  "a": ["300", 3, "8432", 3, 2],
  "inB": "100500",
  "v": {
    "v": [
      {
        "start": {"x": 3, "y": 5},
        "end": {"x": 6, "y": 7}
      },
      {
        "start": {"x": 8, "y": 9},
        "end": {"x": 10, "y": 11}
      }
    ]
  }
}"#;
        let result = parse_signals_json(i.as_bytes(), &ff).unwrap();
        let mut want: HashMap<String, U254> = HashMap::new();
        want.insert("a[0]".to_string(), U254::from_str("300").unwrap());
        want.insert("a[1]".to_string(), U254::from_str("3").unwrap());
        want.insert("a[2]".to_string(), U254::from_str("8432").unwrap());
        want.insert("a[3]".to_string(), U254::from_str("3").unwrap());
        want.insert("a[4]".to_string(), U254::from_str("2").unwrap());
        want.insert("inB".to_string(), U254::from_str("100500").unwrap());
        want.insert("v.v[0].start.x".to_string(), U254::from_str("3").unwrap());
        want.insert("v.v[0].start.y".to_string(), U254::from_str("5").unwrap());
        want.insert("v.v[0].end.x".to_string(), U254::from_str("6").unwrap());
        want.insert("v.v[0].end.y".to_string(), U254::from_str("7").unwrap());
        want.insert("v.v[1].start.x".to_string(), U254::from_str("8").unwrap());
        want.insert("v.v[1].start.y".to_string(), U254::from_str("9").unwrap());
        want.insert("v.v[1].end.x".to_string(), U254::from_str("10").unwrap());
        want.insert("v.v[1].end.y".to_string(), U254::from_str("11").unwrap());
        assert_eq!(want, result);
    }
}
