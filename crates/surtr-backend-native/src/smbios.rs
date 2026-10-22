//! SMBIOS table parsing, written from the DMTF SMBIOS specification (DSP0134).
//! Pure and bounds-checked: malformed firmware tables yield fewer results,
//! never a panic. Only type 17 (Memory Device) is read for now, and its serial
//! number is deliberately skipped.

/// One memory device (SMBIOS type 17) that has a module installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryDevice {
    /// Device locator, for example "DIMM_A2".
    pub slot: Option<String>,
    /// Size in bytes.
    pub size_bytes: Option<u64>,
    /// Configured speed (or rated speed if no configured speed), MT/s.
    pub speed_mts: Option<u32>,
    /// Memory technology, for example "DDR5".
    pub memory_type: Option<&'static str>,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Part number.
    pub part_number: Option<String>,
}

/// One structure: its type, formatted area and strings.
struct Structure<'a> {
    kind: u8,
    data: &'a [u8],
    strings: Vec<&'a [u8]>,
}

impl Structure<'_> {
    fn byte(&self, offset: usize) -> Option<u8> {
        self.data.get(offset).copied()
    }

    fn word(&self, offset: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            self.data.get(offset..offset + 2)?.try_into().ok()?,
        ))
    }

    fn dword(&self, offset: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.data.get(offset..offset + 4)?.try_into().ok()?,
        ))
    }

    /// The string a byte at `offset` refers to (1-based; 0 means none).
    /// Placeholder values firmware often uses count as absent.
    fn string(&self, offset: usize) -> Option<String> {
        let index = usize::from(self.byte(offset)?).checked_sub(1)?;
        let s = String::from_utf8_lossy(self.strings.get(index)?)
            .trim()
            .to_owned();
        let placeholder = [
            "",
            "Not Specified",
            "Unknown",
            "NO DIMM",
            "None",
            "To Be Filled By O.E.M.",
            "Undefined",
        ]
        .iter()
        .any(|p| s.eq_ignore_ascii_case(p));
        (!placeholder).then_some(s)
    }
}

/// Splits a raw SMBIOS table into structures, stopping at the end-of-table
/// marker or at the first malformed structure.
fn structures(table: &[u8]) -> Vec<Structure<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 4 <= table.len() {
        let kind = table[pos];
        let len = usize::from(table[pos + 1]);
        if len < 4 || pos + len > table.len() {
            break;
        }
        let data = &table[pos..pos + len];
        // The string set ends with two NUL bytes.
        let rest = &table[pos + len..];
        let Some(end) = rest.windows(2).position(|w| w == [0, 0]) else {
            break;
        };
        let strings = rest[..end]
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        out.push(Structure {
            kind,
            data,
            strings,
        });
        pos += len + end + 2;
        if kind == 127 {
            break;
        }
    }
    out
}

/// Many firmwares report the manufacturer as a JEDEC ID such as
/// `80CE000080CE` instead of a name. Only well-known IDs are translated;
/// anything else is kept as reported.
fn manufacturer(reported: String) -> String {
    let id = reported.to_ascii_uppercase();
    let known = [
        ("80CE", "Samsung"),
        ("80AD", "SK hynix"),
        ("802C", "Micron"),
    ];
    match known.iter().find(|(code, _)| id.starts_with(code)) {
        Some((_, name)) if id.chars().all(|c| c.is_ascii_hexdigit()) => (*name).to_owned(),
        _ => reported,
    }
}

fn memory_type(code: u8) -> Option<&'static str> {
    Some(match code {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x20 => "HBM",
        0x21 => "HBM2",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        0x24 => "HBM3",
        _ => return None,
    })
}

/// Installed memory modules from a raw SMBIOS structure table.
pub fn memory_devices(table: &[u8]) -> Vec<MemoryDevice> {
    structures(table)
        .into_iter()
        .filter(|s| s.kind == 17)
        .filter_map(|s| {
            let size = match s.word(0x0C)? {
                0 => return None, // empty slot
                0xFFFF => None,   // unknown
                0x7FFF => s.dword(0x1C).map(|mb| u64::from(mb & 0x7FFF_FFFF) << 20),
                v if v & 0x8000 != 0 => Some(u64::from(v & 0x7FFF) << 10),
                v => Some(u64::from(v) << 20),
            };
            let speed = |offset: usize, extended: usize| match s.word(offset)? {
                0 => None,
                0xFFFF => s.dword(extended).filter(|&v| v > 0),
                v => Some(u32::from(v)),
            };
            Some(MemoryDevice {
                slot: s.string(0x10),
                size_bytes: size,
                speed_mts: speed(0x20, 0x58).or_else(|| speed(0x15, 0x54)),
                memory_type: s.byte(0x12).and_then(memory_type),
                manufacturer: s.string(0x17).map(manufacturer),
                part_number: s.string(0x1A),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type 17 structure (length 0x28, SMBIOS 2.8 layout) with strings.
    fn memory_device(
        size: u16,
        speed: u16,
        configured: u16,
        kind: u8,
        strings: &[&str],
    ) -> Vec<u8> {
        let mut s = vec![0u8; 0x28];
        s[0] = 17;
        s[1] = 0x28;
        s[0x0C..0x0E].copy_from_slice(&size.to_le_bytes());
        s[0x10] = 1; // device locator
        s[0x11] = 2; // bank locator
        s[0x12] = kind;
        s[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        s[0x17] = 3; // manufacturer
        s[0x18] = 4; // serial number (must never be read)
        s[0x1A] = 5; // part number
        s[0x20..0x22].copy_from_slice(&configured.to_le_bytes());
        for text in strings {
            s.extend_from_slice(text.as_bytes());
            s.push(0);
        }
        s.push(0);
        s
    }

    fn table() -> Vec<u8> {
        let mut t = Vec::new();
        // Type 0 (BIOS) with one string, to be skipped.
        t.extend_from_slice(&[0, 4, 0, 0]);
        t.extend_from_slice(b"Vendor\0\0");
        t.extend(memory_device(
            16 * 1024,
            5600,
            5200,
            0x22,
            &["DIMM_A1", "BANK 0", "Kingston", "SERIAL123", "KF556C40-16"],
        ));
        // An empty slot.
        t.extend(memory_device(
            0,
            0,
            0,
            0x02,
            &["DIMM_B1", "BANK 1", "Unknown", "X", "Unknown"],
        ));
        // 32 GiB via the extended size field, placeholder manufacturer.
        let mut big = memory_device(
            0x7FFF,
            4800,
            0,
            0x22,
            &["DIMM_A2", "BANK 0", "Not Specified", "X", "P2"],
        );
        big[0x1C..0x20].copy_from_slice(&(32 * 1024u32).to_le_bytes());
        t.extend(big);
        // End of table, without strings.
        t.extend_from_slice(&[127, 4, 0, 0, 0, 0]);
        t
    }

    #[test]
    fn reads_installed_modules() {
        let devices = memory_devices(&table());
        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0],
            MemoryDevice {
                slot: Some("DIMM_A1".into()),
                size_bytes: Some(16 << 30),
                speed_mts: Some(5200),
                memory_type: Some("DDR5"),
                manufacturer: Some("Kingston".into()),
                part_number: Some("KF556C40-16".into()),
            }
        );
        assert_eq!(devices[1].size_bytes, Some(32 << 30));
        assert_eq!(
            devices[1].speed_mts,
            Some(4800),
            "falls back to rated speed"
        );
        assert_eq!(devices[1].manufacturer, None, "placeholder is absent");
    }

    #[test]
    fn jedec_manufacturer_ids() {
        assert_eq!(manufacturer("80CE000080CE".into()), "Samsung");
        assert_eq!(manufacturer("80ad000080ad".into()), "SK hynix");
        assert_eq!(manufacturer("Kingston".into()), "Kingston");
        assert_eq!(manufacturer("0198".into()), "0198");
        assert_eq!(manufacturer("80CE Corp".into()), "80CE Corp");
    }

    #[test]
    fn serial_numbers_never_appear() {
        let text = format!("{:?}", memory_devices(&table()));
        assert!(!text.contains("SERIAL123"));
    }

    // Any truncation of a valid table must not panic.
    #[test]
    fn truncated_tables_are_safe() {
        let t = table();
        for n in 0..t.len() {
            memory_devices(&t[..n]);
        }
        memory_devices(&[17, 2, 0]);
        memory_devices(&[17, 255, 0, 0, 0, 0]);
    }
}
