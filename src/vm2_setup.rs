use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, RwLock};
use crate::field::{FieldOperations, FieldOps};
use crate::vm2::{Component, InputInfo, InputInfoSliceExt, RuntimeError, Template, Type, TypeFieldKind};

/// Initialize signals array with input values from JSON
pub fn init_signals<T: FieldOps, F>(
    inputs_json: impl std::io::Read, ff: &F, types: &[Type],
    input_infos: &[InputInfo],
    component: &mut Component<T>) -> Result<(), Box<dyn std::error::Error>>
where
        for <'a> &'a F: FieldOperations<Type = T> {

    let input_signals = parse_signals_json(inputs_json, ff)?;

    if input_infos.is_empty() {
        return if input_signals.is_empty() {
            Ok(())
        } else {
            Err("input JSON contains signals but circuit has no inputs".into())
        };
    }

    let first_offset = input_infos.min_offset().unwrap();
    let total_inputs = input_infos.get_total_size(types)?;
    let mut signals_set = vec![false; total_inputs];

    for (path, value) in input_signals.iter() {
        let signal_idx = path_to_signal_idx(path, input_infos, types)
            .ok_or_else(|| format!("signal {} is not found in input infos", path))?;

        let local_idx = signal_idx - first_offset;
        if signals_set[local_idx] {
            return Err(format!("duplicate signal at path {}", path).into());
        }
        signals_set[local_idx] = true;
        component.set_signal(signal_idx, *value).map_err(|e| -> Box<dyn Error> { e })?;
    }

    // Check if any input signals were not provided
    if let Some(missing_idx) = signals_set.iter().position(|&s| !s) {
        return Err(format!("missing input signal at offset {}", first_offset + missing_idx).into());
    }

    Ok(())
}

/// Convert a JSON path to signal index
fn path_to_signal_idx(path: &str, input_infos: &[InputInfo], types: &[Type]) -> Option<usize> {
    // Handle root array: "[5]" -> first_offset + 5
    if path.starts_with('[') {
        if let Some(idx) = parse_root_array_index(path) {
            return Some(input_infos.first()?.offset + idx);
        }
    }

    // Find which input this path belongs to
    for info in input_infos {
        if !path.starts_with(&info.name) {
            continue;
        }

        let suffix = &path[info.name.len()..];

        // Suffix must be empty, start with '[', or start with '.'
        if !suffix.is_empty() && !suffix.starts_with('[') && !suffix.starts_with('.') {
            continue;
        }

        if let Some(offset) = calculate_offset_from_suffix(suffix, info, types) {
            return Some(info.offset + offset);
        }
    }

    None
}

/// Parse root array index from path like "[5]" or "[12]"
fn parse_root_array_index(path: &str) -> Option<usize> {
    let path = path.strip_prefix('[')?;
    let end = path.find(']')?;
    // Ensure nothing follows the bracket (or it continues as nested array)
    let after = &path[end + 1..];
    if after.is_empty() {
        path[..end].parse().ok()
    } else {
        None
    }
}

/// Calculate offset within an input from the path suffix
fn calculate_offset_from_suffix(suffix: &str, info: &InputInfo, types: &[Type]) -> Option<usize> {
    if suffix.is_empty() {
        // Single scalar or single bus with no array indexing
        return if info.lengths.is_empty() && info.type_id.is_none() {
            Some(0)
        } else {
            None // Need indices for arrays/buses
        };
    }

    let bus_type = info.type_id.as_ref()
        .and_then(|id| types.iter().find(|t| &t.name == id));

    // Handle input array indexing first
    if !info.lengths.is_empty() {
        // Calculate total size of this input
        let bus_size = bus_type.map(|b| calculate_bus_total_size(b, types)).unwrap_or(1);
        let array_count: usize = info.lengths.iter().product();
        let total_size = array_count * bus_size;

        // Try as flat index first (simple "[N]" with no further access)
        if let Some(flat_idx) = try_parse_flat_index(suffix, total_size) {
            return Some(flat_idx);
        }

        let (array_offset, remaining) = parse_array_indices(suffix, &info.lengths)?;

        if let Some(bus) = bus_type {
            let inner_offset = calculate_bus_offset(remaining, bus, types)?;
            Some(array_offset * bus_size + inner_offset)
        } else {
            // Plain array - remaining should be empty
            if remaining.is_empty() {
                Some(array_offset)
            } else {
                None
            }
        }
    } else if let Some(bus) = bus_type {
        // Single bus instance
        calculate_bus_offset(suffix, bus, types)
    } else {
        // Single scalar - suffix should be empty or "[0]" for compat
        if suffix == "[0]" {
            Some(0)
        } else {
            None
        }
    }
}

/// Try to parse a flat index that spans the full input size
/// Returns Some(idx) if suffix is "[N]" where N < total_size (simple flat index, no further access)
fn try_parse_flat_index(suffix: &str, total_size: usize) -> Option<usize> {
    if !suffix.starts_with('[') {
        return None;
    }
    let close = suffix.find(']')?;
    let after = &suffix[close + 1..];
    if !after.is_empty() {
        return None; // Not a simple flat index (has further access like ".field" or "[N]")
    }
    let idx: usize = suffix[1..close].parse().ok()?;

    if idx < total_size {
        Some(idx)
    } else {
        None
    }
}

/// Parse array indices and return flat offset and remaining suffix
/// Handles both multi-dimensional "[0][1][2]" and flat "[5]" indexing
fn parse_array_indices<'a>(suffix: &'a str, dimensions: &[usize]) -> Option<(usize, &'a str)> {
    if !suffix.starts_with('[') {
        return None;
    }

    // Try parsing as multi-dimensional indices
    let mut remaining = suffix;
    let mut indices = Vec::new();

    while remaining.starts_with('[') {
        let close = remaining.find(']')?;
        let idx: usize = remaining[1..close].parse().ok()?;
        indices.push(idx);
        remaining = &remaining[close + 1..];

        if indices.len() == dimensions.len() {
            break;
        }
    }

    if indices.len() == dimensions.len() {
        // Multi-dimensional indexing
        let flat_idx = indices.iter()
            .zip(dimensions.iter())
            .fold(0, |acc, (&idx, &dim)| acc * dim + idx);
        return Some((flat_idx, remaining));
    }

    // Try as flat index (single bracket with full array offset)
    if indices.len() == 1 {
        let total_size: usize = dimensions.iter().product();
        if indices[0] < total_size {
            // Reparse to get remaining after first bracket only
            let close = suffix.find(']')?;
            return Some((indices[0], &suffix[close + 1..]));
        }
    }

    None
}

/// Calculate offset within a bus type from a path suffix
fn calculate_bus_offset(suffix: &str, bus_type: &Type, types: &[Type]) -> Option<usize> {
    if suffix.is_empty() {
        return Some(0);
    }

    // Handle flat array indexing into bus: "[2]" -> find which field
    if suffix.starts_with('[') {
        let close = suffix.find(']')?;
        let flat_idx: usize = suffix[1..close].parse().ok()?;
        let remaining = &suffix[close + 1..];

        return flat_idx_to_bus_offset(flat_idx, remaining, bus_type, types);
    }

    // Handle field access: ".fieldname..."
    if let Some(field_part) = suffix.strip_prefix('.') {
        let (field_name, rest) = split_field_name(field_part);

        let mut offset = 0;
        for field in &bus_type.fields {
            if field.name == field_name {
                let inner = calculate_field_offset(rest, field, types)?;
                return Some(offset + inner);
            }
            offset += calculate_field_total_size(field, types);
        }
    }

    None
}

/// Convert flat index within a bus to offset
fn flat_idx_to_bus_offset(flat_idx: usize, remaining: &str, bus_type: &Type, types: &[Type]) -> Option<usize> {
    let mut current_offset = 0;

    for field in &bus_type.fields {
        let field_size = calculate_field_total_size(field, types);

        if flat_idx < current_offset + field_size {
            let idx_within = flat_idx - current_offset;
            let inner = calculate_field_offset_by_flat_idx(idx_within, remaining, field, types)?;
            return Some(current_offset + inner);
        }
        current_offset += field_size;
    }

    None
}

/// Calculate offset within a field (handles arrays and nested buses)
fn calculate_field_offset(suffix: &str, field: &crate::vm2::TypeField, types: &[Type]) -> Option<usize> {
    match &field.kind {
        TypeFieldKind::Ff => {
            if field.dims.is_empty() {
                // Scalar - suffix should be empty or "[0]"
                if suffix.is_empty() || suffix == "[0]" {
                    Some(0)
                } else {
                    None
                }
            } else {
                // Array of scalars
                let (array_offset, remaining) = parse_array_indices(suffix, &field.dims)?;
                if remaining.is_empty() {
                    Some(array_offset)
                } else {
                    None
                }
            }
        }
        TypeFieldKind::Bus(bus_idx) => {
            let nested_bus = types.get(*bus_idx)?;
            let bus_size = calculate_bus_total_size(nested_bus, types);

            if field.dims.is_empty() {
                // Single nested bus
                calculate_bus_offset(suffix, nested_bus, types)
            } else {
                // Array of buses
                let (array_offset, remaining) = parse_array_indices(suffix, &field.dims)?;
                let inner = calculate_bus_offset(remaining, nested_bus, types)?;
                Some(array_offset * bus_size + inner)
            }
        }
    }
}

/// Calculate offset within a field using flat index
fn calculate_field_offset_by_flat_idx(
    flat_idx: usize,
    remaining: &str,
    field: &crate::vm2::TypeField,
    types: &[Type]
) -> Option<usize> {
    match &field.kind {
        TypeFieldKind::Ff => {
            if remaining.is_empty() {
                Some(flat_idx)
            } else {
                None
            }
        }
        TypeFieldKind::Bus(bus_idx) => {
            let nested_bus = types.get(*bus_idx)?;
            let bus_size = calculate_bus_total_size(nested_bus, types);

            if field.dims.is_empty() {
                // Single nested bus - recurse into it
                flat_idx_to_bus_offset(flat_idx, remaining, nested_bus, types)
            } else {
                // Array of buses
                let array_idx = flat_idx / bus_size;
                let idx_within_bus = flat_idx % bus_size;
                let inner = flat_idx_to_bus_offset(idx_within_bus, remaining, nested_bus, types)?;
                Some(array_idx * bus_size + inner)
            }
        }
    }
}

/// Split field name from rest of path
fn split_field_name(s: &str) -> (&str, &str) {
    let bracket_pos = s.find('[');
    let dot_pos = s.find('.');

    match (bracket_pos, dot_pos) {
        (Some(b), Some(d)) => {
            let pos = b.min(d);
            (&s[..pos], &s[pos..])
        }
        (Some(b), None) => (&s[..b], &s[b..]),
        (None, Some(d)) => (&s[..d], &s[d..]),
        (None, None) => (s, ""),
    }
}

/// Calculate the total size of a field including array dimensions
fn calculate_field_total_size(field: &crate::vm2::TypeField, types: &[Type]) -> usize {
    let base_size = match &field.kind {
        TypeFieldKind::Ff => 1,
        TypeFieldKind::Bus(bus_idx) => {
            let bus_type = &types[*bus_idx];
            calculate_bus_total_size(bus_type, types)
        }
    };

    if field.dims.is_empty() {
        base_size
    } else {
        base_size * field.dims.iter().product::<usize>()
    }
}

/// Calculate the total size of a bus type
fn calculate_bus_total_size(bus_type: &Type, types: &[Type]) -> usize {
    bus_type.fields.iter()
        .map(|f| calculate_field_total_size(f, types))
        .sum()
}

/// Build the component tree for VM2 execution
pub fn build_component_tree<T: FieldOps>(
    main_template_id: usize, vm_templates: &[Template]) -> Result<Component<T>, RuntimeError> {

    Ok(create_component(main_template_id, 1, vm_templates)?.0)
}

/// Create a component tree and returns the component and the number of signals
/// of self and all its children.
///
/// `template_id` and every subcomponent id come from the decoded artifact and
/// are not trusted, so each is bounds-checked against `vm_templates` before
/// indexing. Once validated here, downstream code can index `templates` by the
/// component's `template_id` without further checks.
fn create_component<T: FieldOps>(
    template_id: usize,
    signals_start: usize, vm_templates: &[Template]) -> Result<(Component<T>, usize), RuntimeError> {

    let t = vm_templates.get(template_id)
        .ok_or(RuntimeError::InvalidTemplateId(template_id))?;
    let mut next_signal_start = signals_start + t.signals_num;
    let mut components = Vec::with_capacity(t.components.len());
    for cmp_tmpl_id in t.components.iter() {
        components.push(match cmp_tmpl_id {
            None => None,
            Some( tmpl_id ) => {
                let (c, signals_num) = create_component(
                    *tmpl_id, next_signal_start, vm_templates)?;
                next_signal_start += signals_num;
                Some(Arc::new(RwLock::new(c)))
            }
        });
    }
    Ok((
        Component::new(
            signals_start,
            template_id,
            components,
            t.number_of_inputs,
            t.signals_num),
        next_signal_start - signals_start
    ))
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
            for (i, v) in vs.iter().enumerate() {
                let new_prefix = if prefix.is_empty() {
                    format!("[{}]", i)
                } else {
                    format!("{}[{}]", prefix, i)
                };
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
        let component_tree: Component<U254> = build_component_tree(6, &vm_templates).unwrap();

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
    fn test_build_component_tree_rejects_invalid_template_id() {
        // A malformed artifact can carry a main_template_id (or subcomponent id)
        // that points past the end of the templates table. Building the tree
        // must surface an error rather than panic while indexing.
        let empty: Vec<Template> = vec![];
        assert!(matches!(
            build_component_tree::<U254>(0, &empty),
            Err(RuntimeError::InvalidTemplateId(0))));

        let parent = Template {
            name: "Parent".to_string(),
            code: vec![],
            signals_num: 1,
            number_of_inputs: 0,
            components: vec![Some(7)], // dangling subcomponent id
            inputs: vec![],
            outputs: vec![],
            ff_variable_names: vec![],
            i64_variable_names: vec![],
        };
        assert!(matches!(
            build_component_tree::<U254>(0, std::slice::from_ref(&parent)),
            Err(RuntimeError::InvalidTemplateId(7))));
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

    #[test]
    fn test_path_to_signal_idx() {
        use crate::vm2::{Type, TypeField, TypeFieldKind};

        // bus_0: { x: Ff[2], y: Ff } → size 3
        // bus_1: { start: Bus(0)[2], end: Bus(0) } → size 9
        let types = vec![
            Type {
                name: "bus_0".to_string(),
                fields: vec![
                    TypeField { name: "x".to_string(), kind: TypeFieldKind::Ff, offset: 0, base_type_size: 1, dims: vec![2] },
                    TypeField { name: "y".to_string(), kind: TypeFieldKind::Ff, offset: 2, base_type_size: 1, dims: vec![] },
                ],
            },
            Type {
                name: "bus_1".to_string(),
                fields: vec![
                    TypeField { name: "start".to_string(), kind: TypeFieldKind::Bus(0), offset: 0, base_type_size: 3, dims: vec![2] },
                    TypeField { name: "end".to_string(), kind: TypeFieldKind::Bus(0), offset: 6, base_type_size: 3, dims: vec![] },
                ],
            },
        ];

        let input_infos = vec![
            InputInfo { name: "a".to_string(), offset: 7, lengths: vec![2, 3], type_id: None },
            InputInfo { name: "b".to_string(), offset: 13, lengths: vec![3, 2], type_id: None },
            InputInfo { name: "c".to_string(), offset: 19, lengths: vec![], type_id: Some("bus_1".to_string()) },
            InputInfo { name: "d".to_string(), offset: 28, lengths: vec![2], type_id: Some("bus_1".to_string()) },
        ];

        // Test "a": Ff[2][3] at offset 7
        assert_eq!(path_to_signal_idx("a[0][0]", &input_infos, &types), Some(7));
        assert_eq!(path_to_signal_idx("a[0][1]", &input_infos, &types), Some(8));
        assert_eq!(path_to_signal_idx("a[0][2]", &input_infos, &types), Some(9));
        assert_eq!(path_to_signal_idx("a[1][0]", &input_infos, &types), Some(10));
        assert_eq!(path_to_signal_idx("a[1][1]", &input_infos, &types), Some(11));
        assert_eq!(path_to_signal_idx("a[1][2]", &input_infos, &types), Some(12));
        // Also test flat indexing
        assert_eq!(path_to_signal_idx("a[0]", &input_infos, &types), Some(7));
        assert_eq!(path_to_signal_idx("a[5]", &input_infos, &types), Some(12));

        // Test "b": Ff[3][2] at offset 13
        assert_eq!(path_to_signal_idx("b[0][0]", &input_infos, &types), Some(13));
        assert_eq!(path_to_signal_idx("b[0][1]", &input_infos, &types), Some(14));
        assert_eq!(path_to_signal_idx("b[1][0]", &input_infos, &types), Some(15));
        assert_eq!(path_to_signal_idx("b[2][1]", &input_infos, &types), Some(18));

        // Test "c": bus_1 at offset 19
        assert_eq!(path_to_signal_idx("c.start[0].x[0]", &input_infos, &types), Some(19));
        assert_eq!(path_to_signal_idx("c.start[0].x[1]", &input_infos, &types), Some(20));
        assert_eq!(path_to_signal_idx("c.start[0].y", &input_infos, &types), Some(21));
        assert_eq!(path_to_signal_idx("c.start[1].x[0]", &input_infos, &types), Some(22));
        assert_eq!(path_to_signal_idx("c.end.x[0]", &input_infos, &types), Some(25));
        assert_eq!(path_to_signal_idx("c.end.y", &input_infos, &types), Some(27));
        // Test flat indexing into bus
        assert_eq!(path_to_signal_idx("c[0]", &input_infos, &types), Some(19));
        assert_eq!(path_to_signal_idx("c[2]", &input_infos, &types), Some(21));
        assert_eq!(path_to_signal_idx("c[8]", &input_infos, &types), Some(27));

        // Test "d": bus_1[2] at offset 28
        assert_eq!(path_to_signal_idx("d[0].start[0].x[0]", &input_infos, &types), Some(28));
        assert_eq!(path_to_signal_idx("d[0].end.y", &input_infos, &types), Some(36));
        assert_eq!(path_to_signal_idx("d[1].start[0].x[0]", &input_infos, &types), Some(37));
        assert_eq!(path_to_signal_idx("d[1].end.y", &input_infos, &types), Some(45));
        // Test flat indexing
        assert_eq!(path_to_signal_idx("d[0][0]", &input_infos, &types), Some(28));
        assert_eq!(path_to_signal_idx("d[0][8]", &input_infos, &types), Some(36));
        assert_eq!(path_to_signal_idx("d[1][0]", &input_infos, &types), Some(37));

        // Test root array indexing
        assert_eq!(path_to_signal_idx("[0]", &input_infos, &types), Some(7));
        assert_eq!(path_to_signal_idx("[12]", &input_infos, &types), Some(19));
        assert_eq!(path_to_signal_idx("[38]", &input_infos, &types), Some(45));
    }

    include!("vm2_setup_tests.rs");
}
