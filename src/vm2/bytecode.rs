use super::{OpCode, RuntimeError};

fn read_byte(code: &[u8], ip: usize) -> Result<u8, RuntimeError> {
    code.get(ip)
        .copied()
        .ok_or(RuntimeError::CodeIndexOutOfBounds)
}

fn read_range(code: &[u8], start: usize, len: usize) -> Result<&[u8], RuntimeError> {
    let end = start
        .checked_add(len)
        .ok_or(RuntimeError::CodeRangeOutOfBounds {
            start,
            len,
            code_len: code.len(),
        })?;
    code.get(start..end)
        .ok_or(RuntimeError::CodeRangeOutOfBounds {
            start,
            len,
            code_len: code.len(),
        })
}

pub(super) fn advance_ip(ip: usize, len: usize) -> Result<usize, RuntimeError> {
    ip.checked_add(len)
        .ok_or(RuntimeError::CodeIndexOutOfBounds)
}

pub(super) fn read_byte_advance(code: &[u8], ip: &mut usize) -> Result<u8, RuntimeError> {
    let byte = read_byte(code, *ip)?;
    *ip = advance_ip(*ip, 1)?;
    Ok(byte)
}

pub(super) fn read_u32_le(code: &[u8], start: usize) -> Result<u32, RuntimeError> {
    let bytes = read_range(code, start, size_of::<u32>())?;
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

fn read_i32_le(code: &[u8], start: usize) -> Result<i32, RuntimeError> {
    let bytes = read_range(code, start, size_of::<i32>())?;
    Ok(i32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

fn read_i64_le(code: &[u8], start: usize) -> Result<i64, RuntimeError> {
    let bytes = read_range(code, start, size_of::<i64>())?;
    Ok(i64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeError::CodeIndexOutOfBounds)?,
    ))
}

pub(super) fn read_i32_advance(code: &[u8], ip: &mut usize) -> Result<i32, RuntimeError> {
    let value = read_i32_le(code, *ip)?;
    *ip = advance_ip(*ip, size_of::<i32>())?;
    Ok(value)
}

pub(super) fn read_i64_advance(code: &[u8], ip: &mut usize) -> Result<i64, RuntimeError> {
    let value = read_i64_le(code, *ip)?;
    *ip = advance_ip(*ip, size_of::<i64>())?;
    Ok(value)
}

pub(super) fn read_range_advance<'a>(
    code: &'a [u8],
    ip: &mut usize,
    len: usize,
) -> Result<&'a [u8], RuntimeError> {
    let bytes = read_range(code, *ip, len)?;
    *ip = advance_ip(*ip, len)?;
    Ok(bytes)
}

pub(super) fn i64_to_usize(value: i64) -> Result<usize, RuntimeError> {
    value
        .try_into()
        .map_err(|_| RuntimeError::I32ToUsizeConversion)
}

pub(super) fn read_usize_advance(code: &[u8], ip: &mut usize) -> Result<usize, RuntimeError> {
    i64_to_usize(read_i64_advance(code, ip)?)
}

pub(super) fn checked_jump_target(
    ip_after_operand: usize,
    offset: i32,
    code_len: usize,
) -> Result<usize, RuntimeError> {
    let target = if offset < 0 {
        ip_after_operand
            .checked_sub(offset.unsigned_abs() as usize)
            .ok_or(RuntimeError::CodeIndexOutOfBounds)?
    } else {
        ip_after_operand
            .checked_add(offset as usize)
            .ok_or(RuntimeError::CodeIndexOutOfBounds)?
    };
    if target <= code_len {
        Ok(target)
    } else {
        Err(RuntimeError::CodeIndexOutOfBounds)
    }
}

pub(super) fn checked_stack_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset).ok_or(RuntimeError::StackOverflow)
}

pub(super) fn checked_memory_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset)
        .ok_or(RuntimeError::MemoryAddressOutOfBounds)
}

pub(super) fn checked_signal_index(base: usize, offset: usize) -> Result<usize, RuntimeError> {
    base.checked_add(offset)
        .ok_or(RuntimeError::SignalIndexOutOfBounds)
}

pub(super) fn read_instruction(code: &[u8], ip: usize) -> Result<OpCode, RuntimeError> {
    let byte = read_byte(code, ip)?;
    OpCode::try_from(byte)
}

// read 4 bytes from the code and return usize and the next instruction pointer
pub(super) fn read_usize32(code: &[u8], ip: usize) -> Result<(usize, usize), RuntimeError> {
    let v = read_u32_le(code, ip)? as usize;
    Ok((v, advance_ip(ip, size_of::<u32>())?))
}
