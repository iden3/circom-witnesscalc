use std::collections::HashMap;
use std::io::Cursor;

use anyhow::anyhow;

use crate::field::{Field, FieldOps};
use crate::vm2::Component;
use crate::vm2_setup::init_signals;
use crate::{vm2, InputSignalsInfo, MAX_GRAPH_V2_INPUT_SIGNALS};

pub(super) fn init_inputs_from_inputs_mapping<T: FieldOps>(
    input_list: &HashMap<String, Vec<T>>,
    inputs_info: &InputSignalsInfo,
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    let mut inputs_len: usize = 1;
    for (offset, len) in inputs_info.values() {
        let idx = checked_input_add(*offset, *len, "offset plus length")?;
        if idx > inputs_len {
            inputs_len = idx;
        }
    }
    let mut inputs = try_reserve_input_vec(inputs_len, "V1 inputs")?;
    inputs.resize(inputs_len, T::zero());
    inputs[0] = T::one();
    let mut inputs_filled = 1;
    for (key, value) in input_list {
        match inputs_info.get(key) {
            None => {
                return Err(anyhow!("Invalid input signal name for the circuit: {}", key).into());
            }
            Some(&(offset, len)) => {
                if len != value.len() {
                    return Err(anyhow!("Invalid input signal {} length: {}", key, len).into());
                }
                for (i, v) in value.iter().enumerate() {
                    inputs[offset + i] = *v;
                    inputs_filled += 1;
                }
            }
        }
    }

    if inputs_filled != inputs_len {
        return Err(anyhow!(
            "Invalid input signal count: {}, expected {}",
            inputs_filled,
            inputs_len
        )
        .into());
    }

    Ok(inputs)
}

pub(super) fn init_inputs_from_v2<T: FieldOps>(
    inputs_json: &str,
    ff: &Field<T>,
    input_info: &[vm2::InputInfo],
    types: &[vm2::Type],
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    let (inputs_size, min_offset) = checked_v2_input_layout(input_info, types)?;
    checked_component_input_count(inputs_size)?;
    let shifted_input_info = shift_input_offsets(input_info, min_offset)?;
    let mut component = Component::new(0, 0, vec![], inputs_size, inputs_size);
    let inputs_cursor = Cursor::new(inputs_json.as_bytes());
    init_signals(
        inputs_cursor,
        ff,
        types,
        &shifted_input_info,
        &mut component,
    )?;
    let mut component_signals = try_reserve_input_vec(inputs_size, "V2 component output")?;
    component.write_all_signals(&mut component_signals);

    let inputs_capacity = checked_input_add(inputs_size, 1, "inputs size plus one")?;
    let mut inputs = try_reserve_input_vec(inputs_capacity, "V2 inputs")?;
    inputs.push(T::one());
    inputs.extend(
        component_signals
            .iter()
            .take(inputs_size)
            .map(|x| x.expect("[assertion] init_signals should not allow None input signals")),
    );

    Ok(inputs)
}

fn checked_v2_input_layout(
    input_info: &[vm2::InputInfo],
    types: &[vm2::Type],
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut inputs_size = 0usize;
    let mut ranges = try_reserve_input_vec(input_info.len(), "V2 input ranges")?;
    for info in input_info {
        let base_type_size = match &info.type_id {
            None => 1,
            Some(type_id) => {
                let type_idx = types
                    .iter()
                    .position(|ty| &ty.name == type_id)
                    .ok_or_else(|| invalid_input_metadata("Unknown input type"))?;
                checked_type_size_by_index(type_idx, types, &mut Vec::new())?
            }
        };
        let array_count = checked_product(&info.lengths, "input dimensions")?;
        let input_size = checked_input_mul(array_count, base_type_size, "input size")?;
        if input_size == 0 {
            return Err(invalid_input_metadata(
                "Input metadata size must be nonzero",
            ));
        }
        inputs_size = checked_input_add(inputs_size, input_size, "total input size")?;
        let end = checked_input_add(info.offset, input_size, "input offset plus size")?;
        ranges.push((info.offset, end));
    }

    ranges.sort_by_key(|(offset, _)| *offset);
    let min_offset = ranges.first().map(|(offset, _)| *offset).unwrap_or(0);
    let mut expected_offset = min_offset;
    for (offset, end) in ranges {
        if offset != expected_offset {
            return Err(invalid_input_metadata(
                "Input metadata offsets must be contiguous and non-overlapping",
            ));
        }
        expected_offset = end;
    }
    let signals_num = checked_input_add(min_offset, inputs_size, "signal count")?;
    if expected_offset != signals_num {
        return Err(invalid_input_metadata(
            "Input metadata span is inconsistent",
        ));
    }
    Ok((inputs_size, min_offset))
}

fn shift_input_offsets(
    input_info: &[vm2::InputInfo],
    min_offset: usize,
) -> Result<Vec<vm2::InputInfo>, Box<dyn std::error::Error>> {
    let mut shifted = try_reserve_input_vec(input_info.len(), "V2 shifted input info")?;
    for info in input_info {
        let mut shifted_info = info.clone();
        shifted_info.offset = info.offset.checked_sub(min_offset).ok_or_else(|| {
            invalid_input_metadata("Input metadata offset is below minimum offset")
        })?;
        shifted.push(shifted_info);
    }
    Ok(shifted)
}

fn checked_component_input_count(signals_num: usize) -> Result<(), Box<dyn std::error::Error>> {
    if signals_num > MAX_GRAPH_V2_INPUT_SIGNALS {
        return Err(invalid_input_metadata(
            "Input metadata V2 input count is too large",
        ));
    }
    Ok(())
}

fn checked_type_size_by_index(
    type_idx: usize,
    types: &[vm2::Type],
    visiting: &mut Vec<usize>,
) -> Result<usize, Box<dyn std::error::Error>> {
    if type_idx >= types.len() {
        return Err(invalid_input_metadata("Bus type index is out of range"));
    }
    if visiting.contains(&type_idx) {
        return Err(invalid_input_metadata("Bus type metadata contains a cycle"));
    }

    visiting.push(type_idx);
    let ty = &types[type_idx];
    let mut total_size = 0usize;
    for field in &ty.fields {
        let base_type_size = checked_field_base_size(field, types, visiting)?;
        if field.base_type_size != base_type_size {
            return Err(invalid_input_metadata(
                "Input metadata base_type_size is inconsistent with field type",
            ));
        }

        let field_size = if field.dims.is_empty() {
            base_type_size
        } else {
            let dim_product = checked_product(&field.dims, "type field dimensions")?;
            checked_input_mul(base_type_size, dim_product, "type field size")?
        };
        total_size = checked_input_add(total_size, field_size, "type size")?;
    }
    visiting.pop();
    Ok(total_size)
}

fn checked_field_base_size(
    field: &vm2::TypeField,
    types: &[vm2::Type],
    visiting: &mut Vec<usize>,
) -> Result<usize, Box<dyn std::error::Error>> {
    match &field.kind {
        vm2::TypeFieldKind::Ff => Ok(1),
        vm2::TypeFieldKind::Bus(bus_idx) => checked_type_size_by_index(*bus_idx, types, visiting),
    }
}

fn checked_product(values: &[usize], context: &str) -> Result<usize, Box<dyn std::error::Error>> {
    values
        .iter()
        .try_fold(1usize, |acc, value| checked_input_mul(acc, *value, context))
}

fn checked_input_add(
    lhs: usize,
    rhs: usize,
    context: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    lhs.checked_add(rhs).ok_or_else(|| {
        invalid_input_metadata(format!("Input metadata {} overflows usize", context))
    })
}

fn checked_input_mul(
    lhs: usize,
    rhs: usize,
    context: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    lhs.checked_mul(rhs).ok_or_else(|| {
        invalid_input_metadata(format!("Input metadata {} overflows usize", context))
    })
}

fn try_reserve_input_vec<T>(
    len: usize,
    context: &str,
) -> Result<Vec<T>, Box<dyn std::error::Error>> {
    let mut vec = Vec::new();
    vec.try_reserve(len).map_err(|_| {
        invalid_input_metadata(format!(
            "Input metadata {} exceeds available memory",
            context
        ))
    })?;
    Ok(vec)
}

fn invalid_input_metadata(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}
