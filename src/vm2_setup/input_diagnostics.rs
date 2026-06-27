use crate::vm2::{InputInfo, Type, TypeField, TypeFieldKind};

/// Build the "unknown input signal ...; expected one of: ..." error.
pub(super) fn unknown_input_signal_message(path: &str, input_infos: &[InputInfo]) -> String {
    format!(
        "unknown input signal {}; expected one of: {}",
        path,
        format_input_names(input_infos)
    )
}

/// Format the circuit's top-level input names for an "expected one of" hint.
/// Sorted so this matches the V1 inputs-mapping diagnostic's ordering.
fn format_input_names(input_infos: &[InputInfo]) -> String {
    if input_infos.is_empty() {
        return "(none)".to_string();
    }
    let mut names: Vec<&str> = input_infos.iter().map(|info| info.name.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

/// Build the missing-input error, preferring a reconstructed path over a bare
/// offset when the offset can be resolved back to an input.
pub(super) fn missing_input_signal_message(
    signal_idx: usize,
    input_infos: &[InputInfo],
    types: &[Type],
) -> String {
    match signal_idx_to_input_path(signal_idx, input_infos, types) {
        Some(path) => format!(
            "missing input signal at path {} (offset {})",
            path, signal_idx
        ),
        None => format!(
            "missing input signal at offset {}; expected one of: {}",
            signal_idx,
            format_input_names(input_infos)
        ),
    }
}

/// Inverse of path_to_signal_idx: find the input owning a signal index and
/// reconstruct its path, or None if no input contains the index.
pub(super) fn signal_idx_to_input_path(
    signal_idx: usize,
    input_infos: &[InputInfo],
    types: &[Type],
) -> Option<String> {
    for info in input_infos {
        let total_size = input_info_total_size(info, types)?;
        if signal_idx >= info.offset && signal_idx < info.offset + total_size {
            return input_offset_to_path(info, signal_idx - info.offset, types);
        }
    }
    None
}

/// Number of signals an input occupies: its array length times its bus size.
fn input_info_total_size(info: &InputInfo, types: &[Type]) -> Option<usize> {
    let base_size = input_bus_type(info, types)
        .map(|bus| calculate_bus_total_size_checked(bus, types))
        .unwrap_or(Some(1))?;
    let array_size = dimensions_total_size(&info.lengths)?;
    base_size.checked_mul(array_size)
}

/// Inverse of calculate_offset_from_suffix: input-relative offset -> path.
fn input_offset_to_path(info: &InputInfo, offset: usize, types: &[Type]) -> Option<String> {
    let bus_type = input_bus_type(info, types);
    let bus_size = bus_type
        .map(|bus| calculate_bus_total_size_checked(bus, types))
        .unwrap_or(Some(1))?;

    if info.lengths.is_empty() {
        if let Some(bus) = bus_type {
            bus_offset_to_path(&info.name, offset, bus, types)
        } else if offset == 0 {
            Some(info.name.clone())
        } else {
            None
        }
    } else {
        let array_offset = offset / bus_size;
        let inner_offset = offset % bus_size;
        let suffix = dimensions_index_suffix(array_offset, &info.lengths)?;
        let prefix = format!("{}{}", info.name, suffix);

        if let Some(bus) = bus_type {
            bus_offset_to_path(&prefix, inner_offset, bus, types)
        } else if inner_offset == 0 {
            Some(prefix)
        } else {
            None
        }
    }
}

/// Resolve an input's bus type from its type_id, if it has one.
fn input_bus_type<'a>(info: &InputInfo, types: &'a [Type]) -> Option<&'a Type> {
    info.type_id
        .as_ref()
        .and_then(|id| types.iter().find(|ty| &ty.name == id))
}

/// Inverse of the row-major fold in parse_array_indices: a flat array index
/// becomes a "[i][j]..." suffix, or None if it is out of range.
fn dimensions_index_suffix(mut flat_idx: usize, dimensions: &[usize]) -> Option<String> {
    if dimensions.is_empty() {
        return Some(String::new());
    }

    let total = dimensions_total_size(dimensions)?;
    if flat_idx >= total {
        return None;
    }

    let mut indexes = Vec::with_capacity(dimensions.len());
    for dim in dimensions.iter().rev() {
        indexes.push(flat_idx % *dim);
        flat_idx /= *dim;
    }
    indexes.reverse();

    Some(
        indexes
            .iter()
            .map(|idx| format!("[{}]", idx))
            .collect::<Vec<_>>()
            .join(""),
    )
}

fn dimensions_total_size(dimensions: &[usize]) -> Option<usize> {
    if dimensions.is_empty() {
        Some(1)
    } else {
        dimensions
            .iter()
            .try_fold(1usize, |acc, dim| acc.checked_mul(*dim))
    }
}

/// Inverse of calculate_bus_offset: a bus-relative offset becomes a path by
/// walking fields in layout order until the one containing the offset is found.
fn bus_offset_to_path(
    prefix: &str,
    offset: usize,
    bus_type: &Type,
    types: &[Type],
) -> Option<String> {
    let mut current_offset = 0;
    for field in &bus_type.fields {
        let field_size = calculate_field_total_size_checked(field, types)?;
        if offset < current_offset + field_size {
            return field_offset_to_path(prefix, field, offset - current_offset, types);
        }
        current_offset += field_size;
    }
    None
}

/// Inverse of calculate_field_offset: a field-relative offset becomes a path,
/// recursing into nested buses and decomposing array indices.
fn field_offset_to_path(
    prefix: &str,
    field: &TypeField,
    offset: usize,
    types: &[Type],
) -> Option<String> {
    let field_prefix = format!("{}.{}", prefix, field.name);
    match &field.kind {
        TypeFieldKind::Ff => {
            if field.dims.is_empty() {
                (offset == 0).then_some(field_prefix)
            } else {
                let suffix = dimensions_index_suffix(offset, &field.dims)?;
                Some(format!("{}{}", field_prefix, suffix))
            }
        }
        TypeFieldKind::Bus(bus_idx) => {
            let nested_bus = types.get(*bus_idx)?;
            let bus_size = calculate_bus_total_size_checked(nested_bus, types)?;

            if field.dims.is_empty() {
                bus_offset_to_path(&field_prefix, offset, nested_bus, types)
            } else {
                let array_offset = offset / bus_size;
                let inner_offset = offset % bus_size;
                let suffix = dimensions_index_suffix(array_offset, &field.dims)?;
                let nested_prefix = format!("{}{}", field_prefix, suffix);
                bus_offset_to_path(&nested_prefix, inner_offset, nested_bus, types)
            }
        }
    }
}

fn calculate_field_total_size_checked(field: &TypeField, types: &[Type]) -> Option<usize> {
    let base_size = match &field.kind {
        TypeFieldKind::Ff => 1,
        TypeFieldKind::Bus(bus_idx) => {
            let bus_type = types.get(*bus_idx)?;
            calculate_bus_total_size_checked(bus_type, types)?
        }
    };

    base_size.checked_mul(dimensions_total_size(&field.dims)?)
}

fn calculate_bus_total_size_checked(bus_type: &Type, types: &[Type]) -> Option<usize> {
    bus_type.fields.iter().try_fold(0usize, |acc, field| {
        acc.checked_add(calculate_field_total_size_checked(field, types)?)
    })
}
