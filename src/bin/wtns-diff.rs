use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use num_bigint::BigUint;
use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 {
        match args[1].as_str() {
            "diff" => return run_diff(&args[2..]),
            "apply" => return run_apply(&args[2..]),
            _ => {}
        }
    }
    print_usage();
    std::process::exit(1);
}

fn run_diff(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 2 {
        print_diff_usage();
        std::process::exit(1);
    }
    let old_path = &args[0];
    let new_path = &args[1];
    let mut output: Box<dyn Write> = if args.len() >= 3 {
        Box::new(File::create(&args[2])?)
    } else {
        Box::new(io::stdout())
    };

    let old_witness = read_wtns(old_path)?;
    let new_witness = read_wtns(new_path)?;

    if old_witness.field_size != new_witness.field_size {
        return Err("Witness files use different field sizes".into());
    }
    if old_witness.values.len() != new_witness.values.len() {
        return Err("Witness files have different number of signals".into());
    }

    let mut idx = 0usize;
    while idx < old_witness.values.len() {
        if old_witness.values[idx] == new_witness.values[idx] {
            idx += 1;
            continue;
        }
        let start = idx;
        let mut chunk_vals: Vec<String> = Vec::new();
        while idx < old_witness.values.len() && old_witness.values[idx] != new_witness.values[idx] {
            let val = hex_string(&new_witness.values[idx]);
            chunk_vals.push(val);
            idx += 1;
        }
        writeln!(
            output,
            "- {} {} {}",
            start,
            chunk_vals.len(),
            chunk_vals.join(" ")
        )?;
    }

    Ok(())
}

fn run_apply(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 {
        print_apply_usage();
        std::process::exit(1);
    }
    let base_path = &args[0];
    let diff_path = &args[1];
    let output_path = &args[2];

    let mut witness = read_wtns(base_path)?;
    let diff_entries = parse_diff(diff_path)?;
    for entry in diff_entries {
        apply_diff_entry(&mut witness, entry)?;
    }
    write_wtns(output_path, &witness)?;
    Ok(())
}

#[derive(Clone)]
struct WitnessFile {
    field_size: usize,
    prime: Vec<u8>,
    values: Vec<Vec<u8>>,
}

#[derive(Debug)]
struct HeaderInfo {
    field_size: usize,
    witness_len: usize,
    prime: Vec<u8>,
}

fn read_wtns(path: &str) -> Result<WitnessFile, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"wtns" {
        return Err("Invalid wtns magic".into());
    }
    let version = reader.read_u32::<LittleEndian>()?;
    if version > 2 {
        return Err("Unsupported wtns version".into());
    }
    let _num_sections = reader.read_u32::<LittleEndian>()?;

    let header = read_header(&mut reader)?;
    let values = read_witness_section(&mut reader, header.witness_len, header.field_size)?;

    Ok(WitnessFile {
        field_size: header.field_size,
        prime: header.prime,
        values,
    })
}

fn read_header<R: Read>(reader: &mut R) -> Result<HeaderInfo, Box<dyn std::error::Error>> {
    let section_type = reader.read_u32::<LittleEndian>()?;
    if section_type != 1 {
        return Err("Expected header section".into());
    }
    let _section_size = reader.read_u64::<LittleEndian>()?;
    let field_size = reader.read_u32::<LittleEndian>()? as usize;
    let mut prime = vec![0u8; field_size];
    reader.read_exact(&mut prime)?;
    let witness_len = reader.read_u32::<LittleEndian>()? as usize;
    Ok(HeaderInfo {
        field_size,
        witness_len,
        prime,
    })
}

fn read_witness_section<R: Read>(
    reader: &mut R,
    witness_len: usize,
    field_size: usize,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let section_type = reader.read_u32::<LittleEndian>()?;
    if section_type != 2 {
        return Err("Expected witness section".into());
    }
    let _section_size = reader.read_u64::<LittleEndian>()?;
    let mut values = Vec::with_capacity(witness_len);
    for _ in 0..witness_len {
        let mut buf = vec![0u8; field_size];
        reader.read_exact(&mut buf)?;
        values.push(buf);
    }
    Ok(values)
}

#[derive(Debug)]
struct DiffEntry {
    offset: usize,
    values: Vec<Vec<u8>>,
}

fn parse_diff(path: &str) -> Result<Vec<DiffEntry>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts: VecDeque<&str> = line.split_whitespace().collect();
        if parts.pop_front() != Some("-") {
            return Err(format!("Invalid diff line: {}", line).into());
        }
        let offset_str = parts
            .pop_front()
            .ok_or_else(|| format!("Missing offset in diff line: {}", line))?;
        let len_str = parts
            .pop_front()
            .ok_or_else(|| format!("Missing length in diff line: {}", line))?;
        let offset: usize = offset_str
            .parse()
            .map_err(|_| format!("Invalid offset: {}", offset_str))?;
        let len: usize = len_str
            .parse()
            .map_err(|_| format!("Invalid length: {}", len_str))?;
        if parts.len() != len {
            return Err(format!(
                "Length mismatch in diff line (expected {} values, got {})",
                len,
                parts.len()
            )
            .into());
        }
        let mut values = Vec::with_capacity(len);
        for token in parts {
            values.push(parse_hex_value(token)?);
        }
        entries.push(DiffEntry { offset, values });
    }
    Ok(entries)
}

fn parse_hex_value(token: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let trimmed = token.trim();
    let hex = if let Some(rest) = trimmed.strip_prefix("0x") {
        rest
    } else {
        trimmed
    };
    let num = BigUint::parse_bytes(hex.as_bytes(), 16)
        .ok_or_else(|| format!("Invalid hex value: {}", token))?;
    let mut bytes = num.to_bytes_le();
    if bytes.is_empty() {
        bytes.push(0);
    }
    Ok(bytes)
}

fn apply_diff_entry(
    witness: &mut WitnessFile,
    entry: DiffEntry,
) -> Result<(), Box<dyn std::error::Error>> {
    let end = entry.offset + entry.values.len();
    if end > witness.values.len() {
        return Err("Diff entry exceeds witness length".into());
    }
    for (i, value) in entry.values.into_iter().enumerate() {
        let mut padded = value;
        if padded.len() > witness.field_size {
            return Err("Diff value exceeds field size".into());
        }
        if padded.len() < witness.field_size {
            padded.resize(witness.field_size, 0);
        }
        witness.values[entry.offset + i] = padded;
    }
    Ok(())
}

fn write_wtns(path: &str, witness: &WitnessFile) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::create(path)?;
    file.write_all(b"wtns")?;
    file.write_u32::<LittleEndian>(2)?;
    file.write_u32::<LittleEndian>(2)?;
    write_header(&mut file, witness)?;
    write_witness_section(&mut file, witness)?;
    Ok(())
}

fn write_header<W: Write>(
    writer: &mut W,
    witness: &WitnessFile,
) -> Result<(), Box<dyn std::error::Error>> {
    writer.write_u32::<LittleEndian>(1)?;
    let sec_size = 4 + witness.field_size as u64 + 4;
    writer.write_u64::<LittleEndian>(sec_size)?;
    writer.write_u32::<LittleEndian>(witness.field_size as u32)?;
    writer.write_all(&witness.prime)?;
    writer.write_u32::<LittleEndian>(witness.values.len() as u32)?;
    Ok(())
}

fn write_witness_section<W: Write>(
    writer: &mut W,
    witness: &WitnessFile,
) -> Result<(), Box<dyn std::error::Error>> {
    writer.write_u32::<LittleEndian>(2)?;
    let sec_size = (witness.values.len() * witness.field_size) as u64;
    writer.write_u64::<LittleEndian>(sec_size)?;
    for value in &witness.values {
        writer.write_all(value)?;
    }
    Ok(())
}

fn hex_string(value_le: &[u8]) -> String {
    let num = BigUint::from_bytes_le(value_le);
    format!("0x{}", num.to_str_radix(16))
}

fn print_usage() {
    eprintln!("Usage:");
    print_diff_usage();
    print_apply_usage();
}

fn print_diff_usage() {
    eprintln!("  wtns-diff diff <old.wtns> <new.wtns> [output.diff]");
}

fn print_apply_usage() {
    eprintln!("  wtns-diff apply <base.wtns> <diff.wtns> <output.wtns>");
}
