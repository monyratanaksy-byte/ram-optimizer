//! Reads memory details from the SMBIOS firmware tables (the data WMI and
//! Task Manager use underneath), so the check-up needs no PowerShell.
//!
//! Only plain byte parsing lives here, so it can be tested anywhere; Windows
//! supplies the table through `GetSystemFirmwareTable`.

#[derive(Debug, PartialEq)]
pub struct MemoryStick {
    pub bytes: u64,
    /// Highest speed the stick supports, in MT/s (0 if unknown).
    pub rated: u32,
    /// Speed it currently runs at, in MT/s (0 if unknown).
    pub configured: u32,
    pub part: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct MemoryInfo {
    pub sticks: Vec<MemoryStick>,
    /// Total memory slots on the board.
    pub slots: u32,
    /// SMBIOS chassis types (laptop, desktop...).
    pub chassis: Vec<u8>,
}

const PHYSICAL_MEMORY_ARRAY: u8 = 16;
const MEMORY_DEVICE: u8 = 17;
const SYSTEM_ENCLOSURE: u8 = 3;
const END_OF_TABLE: u8 = 127;

/// Parses the SMBIOS structure table (the bytes after Windows' 8-byte
/// `RawSMBIOSData` header).
pub fn parse(table: &[u8]) -> MemoryInfo {
    let mut info = MemoryInfo::default();
    let mut at = 0;
    while at + 4 <= table.len() {
        let kind = table[at];
        let length = table[at + 1] as usize;
        if length < 4 || at + length > table.len() {
            break;
        }
        let fields = &table[at..at + length];

        // Each structure is followed by its strings, ending in two NULs.
        let strings_start = at + length;
        let mut end = strings_start;
        while end + 1 < table.len() && !(table[end] == 0 && table[end + 1] == 0) {
            end += 1;
        }
        let strings = &table[strings_start..end.min(table.len())];

        match kind {
            PHYSICAL_MEMORY_ARRAY => {
                if let Some(count) = word(fields, 0x0D) {
                    info.slots += u32::from(count);
                }
            }
            MEMORY_DEVICE => {
                if let Some(stick) = memory_stick(fields, strings) {
                    info.sticks.push(stick);
                }
            }
            SYSTEM_ENCLOSURE => {
                if let Some(&kind) = fields.get(0x05) {
                    info.chassis.push(kind & 0x7F);
                }
            }
            END_OF_TABLE => break,
            _ => {}
        }
        at = end + 2;
    }
    info
}

fn memory_stick(fields: &[u8], strings: &[u8]) -> Option<MemoryStick> {
    const MIB: u64 = 1024 * 1024;
    let size = word(fields, 0x0C)?;
    let bytes = match size {
        0 | 0xFFFF => return None, // empty slot or unknown
        0x7FFF => u64::from(dword(fields, 0x1C)? & 0x7FFF_FFFF) * MIB,
        s if s & 0x8000 != 0 => u64::from(s & 0x7FFF) * 1024,
        s => u64::from(s) * MIB,
    };
    let speed = |short: usize, extended: usize| match word(fields, short) {
        Some(0xFFFF) => dword(fields, extended).unwrap_or(0),
        Some(value) => u32::from(value),
        None => 0,
    };
    Some(MemoryStick {
        bytes,
        rated: speed(0x15, 0x54),
        configured: speed(0x20, 0x58),
        part: fields.get(0x1A).map_or(String::new(), |&index| string_at(strings, index)),
    })
}

fn word(fields: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(fields.get(offset..offset + 2)?.try_into().ok()?))
}

fn dword(fields: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(fields.get(offset..offset + 4)?.try_into().ok()?))
}

/// SMBIOS strings are numbered from 1; 0 means "none".
fn string_at(strings: &[u8], index: u8) -> String {
    if index == 0 {
        return String::new();
    }
    strings
        .split(|&b| b == 0)
        .nth(usize::from(index) - 1)
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .unwrap_or_default()
}

/// Formats the result as check-up fact lines (see `checkup::evaluate`).
pub fn facts(info: &MemoryInfo) -> String {
    let mut lines = String::new();
    for stick in &info.sticks {
        lines += &format!("stick|{}|{}|{}|{}\n", stick.bytes, stick.rated, stick.configured, stick.part);
    }
    if info.slots > 0 {
        lines += &format!("slots|{}\n", info.slots);
    }
    if !info.chassis.is_empty() {
        let types: Vec<String> = info.chassis.iter().map(u8::to_string).collect();
        lines += &format!("chassis|{}\n", types.join(","));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds one SMBIOS structure: header, fields, then strings.
    fn structure(kind: u8, length: u8, mut set: impl FnMut(&mut Vec<u8>), strings: &[&str]) -> Vec<u8> {
        let mut fields = vec![0u8; usize::from(length)];
        fields[0] = kind;
        fields[1] = length;
        set(&mut fields);
        let mut out = fields;
        for s in strings {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        if strings.is_empty() {
            out.push(0);
        }
        out.push(0);
        out
    }

    fn put_word(fields: &mut [u8], at: usize, value: u16) {
        fields[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn reads_sticks_slots_and_chassis() {
        let mut table = Vec::new();
        table.extend(structure(SYSTEM_ENCLOSURE, 0x15, |f| f[0x05] = 0x0A, &["Acer"]));
        table.extend(structure(PHYSICAL_MEMORY_ARRAY, 0x17, |f| put_word(f, 0x0D, 2), &[]));
        table.extend(structure(
            MEMORY_DEVICE,
            0x28,
            |f| {
                put_word(f, 0x0C, 16 * 1024); // 16 GB in MB
                put_word(f, 0x15, 5600);
                f[0x1A] = 2; // part number is the 2nd string
                put_word(f, 0x20, 5200);
            },
            &["Samsung", "M425R2GA3BB0-CWMOD"],
        ));
        // An empty slot reports size 0 and must be skipped.
        table.extend(structure(MEMORY_DEVICE, 0x28, |_| {}, &[]));
        table.extend(structure(END_OF_TABLE, 4, |_| {}, &[]));

        let info = parse(&table);
        assert_eq!(info.slots, 2);
        assert_eq!(info.chassis, vec![10]);
        assert_eq!(
            info.sticks,
            vec![MemoryStick {
                bytes: 16 * 1024 * 1024 * 1024,
                rated: 5600,
                configured: 5200,
                part: "M425R2GA3BB0-CWMOD".into(),
            }]
        );
        assert_eq!(
            facts(&info),
            "stick|17179869184|5600|5200|M425R2GA3BB0-CWMOD\nslots|2\nchassis|10\n"
        );
    }

    #[test]
    fn survives_truncated_tables() {
        assert_eq!(parse(&[17, 40, 0]), MemoryInfo::default());
        assert_eq!(parse(&[]), MemoryInfo::default());
    }
}
